use bit_vec::{self, BitVec};
use ff::{Field, PrimeField, PrimeFieldRepr, ScalarEngine};
use futures::Future;
use groupy::{CurveAffine, CurveProjective};
use std::io;
use std::iter;
use std::sync::{Arc, Mutex};

use super::multicore::Worker;
use super::SynthesisError;
use crate::gpu;

/// An object that builds a source of bases.
pub trait SourceBuilder<G: CurveAffine>: Send + Sync + 'static + Clone {
    type Source: Source<G>;

    fn new(self) -> Self::Source;
    fn get(self) -> (Arc<Vec<G>>, usize);
}

/// A source of bases, like an iterator.
pub trait Source<G: CurveAffine> {
    /// Parses the element from the source. Fails if the point is at infinity.
    fn add_assign_mixed(
        &mut self,
        to: &mut <G as CurveAffine>::Projective,
    ) -> Result<(), SynthesisError>;

    /// Skips `amt` elements from the source, avoiding deserialization.
    fn skip(&mut self, amt: usize) -> Result<(), SynthesisError>;
}

impl<G: CurveAffine> SourceBuilder<G> for (Arc<Vec<G>>, usize) {
    type Source = (Arc<Vec<G>>, usize);

    fn new(self) -> (Arc<Vec<G>>, usize) {
        (self.0.clone(), self.1)
    }

    fn get(self) -> (Arc<Vec<G>>, usize) {
        (self.0.clone(), self.1)
    }
}

impl<G: CurveAffine> Source<G> for (Arc<Vec<G>>, usize) {
    fn add_assign_mixed(
        &mut self,
        to: &mut <G as CurveAffine>::Projective,
    ) -> Result<(), SynthesisError> {
        if self.0.len() <= self.1 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "expected more bases from source",
            )
            .into());
        }

        if self.0[self.1].is_zero() {
            return Err(SynthesisError::UnexpectedIdentity);
        }

        to.add_assign_mixed(&self.0[self.1]);

        self.1 += 1;

        Ok(())
    }

    fn skip(&mut self, amt: usize) -> Result<(), SynthesisError> {
        if self.0.len() <= self.1 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "expected more bases from source",
            )
            .into());
        }

        self.1 += amt;

        Ok(())
    }
}

pub trait QueryDensity {
    /// Returns whether the base exists.
    type Iter: Iterator<Item = bool>;

    fn iter(self) -> Self::Iter;
    fn get_query_size(self) -> Option<usize>;
}

#[derive(Clone)]
pub struct FullDensity;

impl AsRef<FullDensity> for FullDensity {
    fn as_ref(&self) -> &FullDensity {
        self
    }
}

impl<'a> QueryDensity for &'a FullDensity {
    type Iter = iter::Repeat<bool>;

    fn iter(self) -> Self::Iter {
        iter::repeat(true)
    }

    fn get_query_size(self) -> Option<usize> {
        None
    }
}

pub struct DensityTracker {
    bv: BitVec,
    total_density: usize,
}

impl<'a> QueryDensity for &'a DensityTracker {
    type Iter = bit_vec::Iter<'a>;

    fn iter(self) -> Self::Iter {
        self.bv.iter()
    }

    fn get_query_size(self) -> Option<usize> {
        Some(self.bv.len())
    }
}

impl DensityTracker {
    pub fn new() -> DensityTracker {
        DensityTracker {
            bv: BitVec::new(),
            total_density: 0,
        }
    }

    pub fn add_element(&mut self) {
        self.bv.push(false);
    }

    pub fn inc(&mut self, idx: usize) {
        if !self.bv.get(idx).unwrap() {
            self.bv.set(idx, true);
            self.total_density += 1;
        }
    }

    pub fn get_total_density(&self) -> usize {
        self.total_density
    }
}

fn multiexp_inner<Q, D, G, S>(
    pool: &Worker,
    bases: S,
    density_map: D,
    exponents: Arc<Vec<<<G::Engine as ScalarEngine>::Fr as PrimeField>::Repr>>,
    mut skip: u32,
    c: u32,
    handle_trivial: bool,
) -> Box<dyn Future<Item = <G as CurveAffine>::Projective, Error = SynthesisError>>
where
    for<'a> &'a Q: QueryDensity,
    D: Send + Sync + 'static + Clone + AsRef<Q>,
    G: CurveAffine,
    S: SourceBuilder<G>,
{
    // Perform this region of the multiexp
    let this = {
        let bases = bases.clone();
        let exponents = exponents.clone();
        let density_map = density_map.clone();

        pool.compute(move || {
            // Accumulate the result
            let mut acc = G::Projective::zero();

            // Build a source for the bases
            let mut bases = bases.new();

            // Create space for the buckets
            let mut buckets = vec![<G as CurveAffine>::Projective::zero(); (1 << c) - 1];

            let zero = <G::Engine as ScalarEngine>::Fr::zero().into_repr();
            let one = <G::Engine as ScalarEngine>::Fr::one().into_repr();

            // Sort the bases into buckets
            for (&exp, density) in exponents.iter().zip(density_map.as_ref().iter()) {
                if density {
                    if exp == zero {
                        bases.skip(1)?;
                    } else if exp == one {
                        if handle_trivial {
                            bases.add_assign_mixed(&mut acc)?;
                        } else {
                            bases.skip(1)?;
                        }
                    } else {
                        let mut exp = exp;
                        exp.shr(skip);
                        let exp = exp.as_ref()[0] % (1 << c);

                        if exp != 0 {
                            bases.add_assign_mixed(&mut buckets[(exp - 1) as usize])?;
                        } else {
                            bases.skip(1)?;
                        }
                    }
                }
            }

            // Summation by parts
            // e.g. 3a + 2b + 1c = a +
            //                    (a) + b +
            //                    ((a) + b) + c
            let mut running_sum = G::Projective::zero();
            for exp in buckets.into_iter().rev() {
                running_sum.add_assign(&exp);
                acc.add_assign(&running_sum);
            }

            Ok(acc)
        })
    };

    skip += c;

    if skip >= <G::Engine as ScalarEngine>::Fr::NUM_BITS {
        // There isn't another region.
        Box::new(this)
    } else {
        // There's another region more significant. Calculate and join it with
        // this region recursively.
        Box::new(
            this.join(multiexp_inner(
                pool,
                bases,
                density_map,
                exponents,
                skip,
                c,
                false,
            ))
            .map(move |(this, mut higher)| {
                for _ in 0..c {
                    higher.double();
                }

                higher.add_assign(&this);

                higher
            }),
        )
    }
}

/// Perform multi-exponentiation. The caller is responsible for ensuring the
/// query size is the same as the number of exponents.
pub fn multiexp<Q, D, G, S>(
    pool: &Worker,
    bases: S,
    density_map: D,
    exponents: Arc<Vec<<<G::Engine as ScalarEngine>::Fr as PrimeField>::Repr>>,
    kern: &mut Option<gpu::MultiexpKernel<G::Engine>>,
) -> Box<dyn Future<Item = <G as CurveAffine>::Projective, Error = SynthesisError>>
where
    for<'a> &'a Q: QueryDensity,
    D: Send + Sync + 'static + Clone + AsRef<Q>,
    G: CurveAffine,
    G::Engine: paired::Engine,
    S: SourceBuilder<G>,
{
    if let Some(ref mut k) = kern {
        let mut exps = vec![exponents[0]; exponents.len()];
        let mut n = 0;
        for (&e, d) in exponents.iter().zip(density_map.as_ref().iter()) {
            if d {
                exps[n] = e;
                n += 1;
            }
        }

        let (bss, skip) = bases.get();
        let result = k.multiexp(bss, Arc::new(exps), skip, n);

        return Box::new(pool.compute(move || match result {
            Ok(p) => Ok(p),
            Err(e) => Err(SynthesisError::from(e)),
        }));
    }

    let c = if exponents.len() < 32 {
        3u32
    } else {
        (f64::from(exponents.len() as u32)).ln().ceil() as u32
    };

    if let Some(query_size) = density_map.as_ref().get_query_size() {
        // If the density map has a known query size, it should not be
        // inconsistent with the number of exponents.

        assert!(query_size == exponents.len());
    }

    multiexp_inner(pool, bases, density_map, exponents, 0, c, true)
}

#[cfg(feature = "pairing")]
#[test]
fn test_with_bls12() {
    fn naive_multiexp<G: CurveAffine>(
        bases: Arc<Vec<G>>,
        exponents: Arc<Vec<<G::Scalar as PrimeField>::Repr>>,
    ) -> G::Projective {
        assert_eq!(bases.len(), exponents.len());

        let mut acc = G::Projective::zero();

        for (base, exp) in bases.iter().zip(exponents.iter()) {
            acc.add_assign(&base.mul(*exp));
        }

        acc
    }

    use paired::{bls12_381::Bls12, Engine};
    use rand;

    const SAMPLES: usize = 1 << 14;

    let rng = &mut rand::thread_rng();
    let v = Arc::new(
        (0..SAMPLES)
            .map(|_| <Bls12 as ScalarEngine>::Fr::random(rng).into_repr())
            .collect::<Vec<_>>(),
    );
    let g = Arc::new(
        (0..SAMPLES)
            .map(|_| <Bls12 as Engine>::G1::random(rng).into_affine())
            .collect::<Vec<_>>(),
    );

    let naive = naive_multiexp(g.clone(), v.clone());

    let pool = Worker::new();

    let fast = multiexp(&pool, (g, 0), FullDensity, v).wait().unwrap();

    assert_eq!(naive, fast);
}

lazy_static::lazy_static! {
    static ref GPU_MULTIEXP_SUPPORTED: Mutex<Option<bool>> = { Mutex::new(None) };
}

use std::env;
pub fn gpu_multiexp_supported<E>() -> Result<gpu::MultiexpKernel<E>, SynthesisError>
where
    E: paired::Engine,
{
    const TEST_SIZE: u32 = 1024;
    let pool = Worker::new();
    let rng = &mut rand::thread_rng();
    let mut kern = Some(gpu::MultiexpKernel::<E>::create()?);

    // Checking the correctness of GPU results can be time consuming. User can disable this
    // feature using BELLMAN_GPU_NO_CHECK flag.
    if env::var("BELLMAN_GPU_NO_CHECK").is_ok() {
        return Ok(kern.unwrap());
    }

    let res = {
        let mut supported = GPU_MULTIEXP_SUPPORTED.lock().unwrap();
        if let Some(res) = *supported {
            res
        } else {
            let bases_g1 = Arc::new(
                (0..TEST_SIZE)
                    .map(|_| E::G1::random(rng).into_affine())
                    .collect::<Vec<_>>(),
            );
            let bases_g2 = Arc::new(
                (0..TEST_SIZE)
                    .map(|_| E::G2::random(rng).into_affine())
                    .collect::<Vec<_>>(),
            );
            let exps = Arc::new(
                (0..TEST_SIZE)
                    .map(|_| E::Fr::random(rng).into_repr())
                    .collect::<Vec<_>>(),
            );
            let gpu_g1 = multiexp(
                &pool,
                (bases_g1.clone(), 0),
                FullDensity,
                exps.clone(),
                &mut kern,
            )
            .wait()?;
            let cpu_g1 =
                multiexp(&pool, (bases_g1, 0), FullDensity, exps.clone(), &mut None).wait()?;
            let gpu_g2 = multiexp(
                &pool,
                (bases_g2.clone(), 0),
                FullDensity,
                exps.clone(),
                &mut kern,
            )
            .wait()?;
            let cpu_g2 = multiexp(&pool, (bases_g2, 0), FullDensity, exps, &mut None).wait()?;
            let res = cpu_g1 == gpu_g1 && cpu_g2 == gpu_g2;
            *supported = Some(res);
            res
        }
    };
    if res {
        Ok(kern.unwrap())
    } else {
        Err(SynthesisError::from(gpu::GPUError {
            msg: "GPU Multiexp not supported!".to_string(),
        }))
    }
}

#[cfg(feature = "gpu-test")]
#[test]
pub fn gpu_multiexp_consistency() {
    use paired::bls12_381::Bls12;
    use std::time::Instant;

    const CHUNK_SIZE: usize = 1048576;
    const MAX_LOG_D: usize = 20;
    const START_LOG_D: usize = 10;
    let mut kern = gpu::MultiexpKernel::<Bls12>::create().ok();
    if kern.is_none() {
        panic!("Cannot initialize kernel!");
    }
    let pool = Worker::new();

    let rng = &mut rand::thread_rng();

    let mut bases = (0..(1 << 10))
        .map(|_| <Bls12 as paired::Engine>::G1::random(rng).into_affine())
        .collect::<Vec<_>>();
    for _ in 10..START_LOG_D {
        bases = [bases.clone(), bases.clone()].concat();
    }

    for log_d in START_LOG_D..(MAX_LOG_D + 1) {
        let g = Arc::new(bases.clone());

        let samples = 1 << log_d;
        println!("Testing Multiexp for {} elements...", samples);

        let v = Arc::new(
            (0..samples)
                .map(|_| <Bls12 as ScalarEngine>::Fr::random(rng).into_repr())
                .collect::<Vec<_>>(),
        );

        let mut now = Instant::now();
        let gpu = multiexp(&pool, (g.clone(), 0), FullDensity, v.clone(), &mut kern)
            .wait()
            .unwrap();
        let gpu_dur = now.elapsed().as_secs() * 1000 as u64 + now.elapsed().subsec_millis() as u64;
        println!("GPU took {}ms.", gpu_dur);

        now = Instant::now();
        let cpu = multiexp(&pool, (g.clone(), 0), FullDensity, v.clone(), &mut None)
            .wait()
            .unwrap();
        let cpu_dur = now.elapsed().as_secs() * 1000 as u64 + now.elapsed().subsec_millis() as u64;
        println!("CPU took {}ms.", cpu_dur);

        println!("Speedup: x{}", cpu_dur as f32 / gpu_dur as f32);

        assert_eq!(cpu, gpu);

        println!("============================");

        bases = [bases.clone(), bases.clone()].concat();
    }
}
