use rand_core::RngCore;

use std::sync::Arc;

use ff::{Field, PrimeField};
use futures::Future;
use groupy::{CurveAffine, CurveProjective};
use log::info;
use paired::Engine;
#[cfg(feature = "gpu")]
use fs2::FileExt;

use super::{ParameterSource, Proof};
use crate::domain::{gpu_fft_supported, EvaluationDomain, Scalar};
#[cfg(feature = "gpu")]
use crate::gpu;
use crate::multicore::Worker;
use crate::multiexp::{gpu_multiexp_supported, multiexp, DensityTracker, FullDensity};
use crate::{Circuit, ConstraintSystem, Index, LinearCombination, SynthesisError, Variable};

// We check to see if another higher priority process needs to use
// the GPU for each multiexp
#[cfg(not(feature = "gpu"))]
macro_rules! check_for_higher_prio {
    () => {
        true
    };
}

#[cfg(feature = "gpu")]
macro_rules! check_for_higher_prio {
    () => {
        gpu::gpu_is_not_acquired().unwrap_or(false);
    };
}

fn eval<E: Engine>(
    lc: &LinearCombination<E>,
    mut input_density: Option<&mut DensityTracker>,
    mut aux_density: Option<&mut DensityTracker>,
    input_assignment: &[E::Fr],
    aux_assignment: &[E::Fr],
) -> E::Fr {
    let mut acc = E::Fr::zero();

    for &(index, coeff) in lc.0.iter() {
        let mut tmp;

        match index {
            Variable(Index::Input(i)) => {
                tmp = input_assignment[i];
                if let Some(ref mut v) = input_density {
                    v.inc(i);
                }
            }
            Variable(Index::Aux(i)) => {
                tmp = aux_assignment[i];
                if let Some(ref mut v) = aux_density {
                    v.inc(i);
                }
            }
        }

        if coeff == E::Fr::one() {
            acc.add_assign(&tmp);
        } else {
            tmp.mul_assign(&coeff);
            acc.add_assign(&tmp);
        }
    }

    acc
}

struct ProvingAssignment<E: Engine> {
    // Density of queries
    a_aux_density: DensityTracker,
    b_input_density: DensityTracker,
    b_aux_density: DensityTracker,

    // Evaluations of A, B, C polynomials
    a: Vec<Scalar<E>>,
    b: Vec<Scalar<E>>,
    c: Vec<Scalar<E>>,

    // Assignments of variables
    input_assignment: Vec<E::Fr>,
    aux_assignment: Vec<E::Fr>,
}

impl<E: Engine> ConstraintSystem<E> for ProvingAssignment<E> {
    type Root = Self;

    fn alloc<F, A, AR>(&mut self, _: A, f: F) -> Result<Variable, SynthesisError>
    where
        F: FnOnce() -> Result<E::Fr, SynthesisError>,
        A: FnOnce() -> AR,
        AR: Into<String>,
    {
        self.aux_assignment.push(f()?);
        self.a_aux_density.add_element();
        self.b_aux_density.add_element();

        Ok(Variable(Index::Aux(self.aux_assignment.len() - 1)))
    }

    fn alloc_input<F, A, AR>(&mut self, _: A, f: F) -> Result<Variable, SynthesisError>
    where
        F: FnOnce() -> Result<E::Fr, SynthesisError>,
        A: FnOnce() -> AR,
        AR: Into<String>,
    {
        self.input_assignment.push(f()?);
        self.b_input_density.add_element();

        Ok(Variable(Index::Input(self.input_assignment.len() - 1)))
    }

    fn enforce<A, AR, LA, LB, LC>(&mut self, _: A, a: LA, b: LB, c: LC)
    where
        A: FnOnce() -> AR,
        AR: Into<String>,
        LA: FnOnce(LinearCombination<E>) -> LinearCombination<E>,
        LB: FnOnce(LinearCombination<E>) -> LinearCombination<E>,
        LC: FnOnce(LinearCombination<E>) -> LinearCombination<E>,
    {
        let a = a(LinearCombination::zero());
        let b = b(LinearCombination::zero());
        let c = c(LinearCombination::zero());

        self.a.push(Scalar(eval(
            &a,
            // Inputs have full density in the A query
            // because there are constraints of the
            // form x * 0 = 0 for each input.
            None,
            Some(&mut self.a_aux_density),
            &self.input_assignment,
            &self.aux_assignment,
        )));
        self.b.push(Scalar(eval(
            &b,
            Some(&mut self.b_input_density),
            Some(&mut self.b_aux_density),
            &self.input_assignment,
            &self.aux_assignment,
        )));
        self.c.push(Scalar(eval(
            &c,
            // There is no C polynomial query,
            // though there is an (beta)A + (alpha)B + C
            // query for all aux variables.
            // However, that query has full density.
            None,
            None,
            &self.input_assignment,
            &self.aux_assignment,
        )));
    }

    fn push_namespace<NR, N>(&mut self, _: N)
    where
        NR: Into<String>,
        N: FnOnce() -> NR,
    {
        // Do nothing; we don't care about namespaces in this context.
    }

    fn pop_namespace(&mut self) {
        // Do nothing; we don't care about namespaces in this context.
    }

    fn get_root(&mut self) -> &mut Self::Root {
        self
    }
}

pub fn create_random_proof<E, C, R, P: ParameterSource<E>>(
    circuit: C,
    params: P,
    rng: &mut R,
) -> Result<Proof<E>, SynthesisError>
where
    E: Engine,
    C: Circuit<E>,
    R: RngCore,
{
    let r = E::Fr::random(rng);
    let s = E::Fr::random(rng);

    create_proof::<E, C, P>(circuit, params, r, s)
}

pub fn create_proof<E, C, P: ParameterSource<E>>(
    circuit: C,
    mut params: P,
    r: E::Fr,
    s: E::Fr,
) -> Result<Proof<E>, SynthesisError>
where
    E: Engine,
    C: Circuit<E>,
{
    #[cfg(feature = "gpu")]
    let lock = gpu::get_lock_file()?;

    let mut prover = ProvingAssignment {
        a_aux_density: DensityTracker::new(),
        b_input_density: DensityTracker::new(),
        b_aux_density: DensityTracker::new(),
        a: vec![],
        b: vec![],
        c: vec![],
        input_assignment: vec![],
        aux_assignment: vec![],
    };

    prover.alloc_input(|| "", || Ok(E::Fr::one()))?;

    circuit.synthesize(&mut prover)?;

    for i in 0..prover.input_assignment.len() {
        prover.enforce(|| "", |lc| lc + Variable(Index::Input(i)), |lc| lc, |lc| lc);
    }

    let worker = Worker::new();

    let vk = params.get_vk(prover.input_assignment.len())?;

    let n = prover.a.len();
    let mut log_d = 0u32;
    while (1 << log_d) < n {
        log_d += 1;
    }

    let a = {
        let mut fft_kern = gpu_fft_supported::<E>(log_d).ok();
        if fft_kern.is_some() {
            info!("GPU FFT is supported!");
        } else {
            info!("GPU FFT is NOT supported!");
        }

        let mut a = EvaluationDomain::from_coeffs(prover.a)?;
        let mut b = EvaluationDomain::from_coeffs(prover.b)?;
        let mut c = EvaluationDomain::from_coeffs(prover.c)?;

        a.ifft(&worker, &mut fft_kern)?;
        a.coset_fft(&worker, &mut fft_kern)?;
        b.ifft(&worker, &mut fft_kern)?;
        b.coset_fft(&worker, &mut fft_kern)?;
        c.ifft(&worker, &mut fft_kern)?;
        c.coset_fft(&worker, &mut fft_kern)?;

        a.mul_assign(&worker, &b);
        drop(b);
        a.sub_assign(&worker, &c);
        drop(c);
        a.divide_by_z_on_coset(&worker, &mut fft_kern)?;
        a.icoset_fft(&worker, &mut fft_kern)?;
        let mut a = a.into_coeffs();
        let a_len = a.len() - 1;
        a.truncate(a_len);
        // TODO: parallelize if it's even helpful
        Arc::new(a.into_iter().map(|s| s.0.into_repr()).collect::<Vec<_>>())
    };

    let mut multiexp_kern = gpu_multiexp_supported::<E>().ok();
    if multiexp_kern.is_some() {
        info!("GPU Multiexp is supported!");
    } else {
        info!("GPU Multiexp is NOT supported!");
    }

    let mut keep_cpu = false;

    let h = if !check_for_higher_prio!() || keep_cpu {
        #[cfg(feature = "gpu")]
        {
            info!("Multiexp 1 Prover found acquire lock, switching to CPU");
            // Free the incoming process to use the GPU
            if !keep_cpu {
                keep_cpu = true;
                lock.unlock()?;
            }
        }
        multiexp(&worker, params.get_h(a.len())?, FullDensity, a, &mut None)
    } else {
        info!("Multiexp 1 Prover NO acquire lock, keeping GPU");
        multiexp(
            &worker,
            params.get_h(a.len())?,
            FullDensity,
            a,
            &mut multiexp_kern,
        )
    };

    // TODO: parallelize if it's even helpful
    let input_assignment = Arc::new(
        prover
            .input_assignment
            .into_iter()
            .map(|s| s.into_repr())
            .collect::<Vec<_>>(),
    );
    let aux_assignment = Arc::new(
        prover
            .aux_assignment
            .into_iter()
            .map(|s| s.into_repr())
            .collect::<Vec<_>>(),
    );

    let l = if !check_for_higher_prio!() || keep_cpu {
        #[cfg(feature = "gpu")]
        {
            info!("Multiexp 2 Prover found acquire lock, switching to CPU");
            // Free the incoming process to use the GPU
            if !keep_cpu {
                keep_cpu = true;
                lock.unlock()?;
            }
        }
        multiexp(
            &worker,
            params.get_l(aux_assignment.len())?,
            FullDensity,
            aux_assignment.clone(),
            &mut None,
        )
    } else {
        info!("Multiexp 2 Prover NO acquire lock, keeping GPU");
        multiexp(
            &worker,
            params.get_l(aux_assignment.len())?,
            FullDensity,
            aux_assignment.clone(),
            &mut multiexp_kern,
        )
    };

    let a_aux_density_total = prover.a_aux_density.get_total_density();

    let (a_inputs_source, a_aux_source) =
        params.get_a(input_assignment.len(), a_aux_density_total)?;

    let a_inputs = if !check_for_higher_prio!() || keep_cpu {
        #[cfg(feature = "gpu")]
        {
            info!("Multiexp 3 Prover found acquire lock, switching to CPU");
            // Free the incoming process to use the GPU
            if !keep_cpu {
                keep_cpu = true;
                lock.unlock()?;
            }
        }
        multiexp(
            &worker,
            a_inputs_source,
            FullDensity,
            input_assignment.clone(),
            &mut None,
        )
    } else {
        info!("Multiexp 3 Prover NO acquire lock, keeping GPU");
        multiexp(
            &worker,
            a_inputs_source,
            FullDensity,
            input_assignment.clone(),
            &mut multiexp_kern,
        )
    };

    let a_aux = if !check_for_higher_prio!() || keep_cpu {
        #[cfg(feature = "gpu")]
        {
            info!("Multiexp 4 Prover found acquire lock, switching to CPU");
            // Free the incoming process to use the GPU
            if !keep_cpu {
                keep_cpu = true;
                lock.unlock()?;
            }
        }
        multiexp(
            &worker,
            a_aux_source,
            Arc::new(prover.a_aux_density),
            aux_assignment.clone(),
            &mut None,
        )
    } else {
        info!("Multiexp 4 Prover NO acquire lock, keeping GPU");
        multiexp(
            &worker,
            a_aux_source,
            Arc::new(prover.a_aux_density),
            aux_assignment.clone(),
            &mut multiexp_kern,
        )
    };

    let b_input_density = Arc::new(prover.b_input_density);
    let b_input_density_total = b_input_density.get_total_density();
    let b_aux_density = Arc::new(prover.b_aux_density);
    let b_aux_density_total = b_aux_density.get_total_density();

    let (b_g1_inputs_source, b_g1_aux_source) =
        params.get_b_g1(b_input_density_total, b_aux_density_total)?;

    let b_g1_inputs = if !check_for_higher_prio!() || keep_cpu {
        #[cfg(feature = "gpu")]
        {
            info!("Multiexp 5 Prover found acquire lock, switching to CPU");
            // Free the incoming process to use the GPU
            if !keep_cpu {
                keep_cpu = true;
                lock.unlock()?;
            }
        }
        multiexp(
            &worker,
            b_g1_inputs_source,
            b_input_density.clone(),
            input_assignment.clone(),
            &mut None,
        )
    } else {
        info!("Multiexp 5 Prover NO acquire lock, keeping GPU");
        multiexp(
            &worker,
            b_g1_inputs_source,
            b_input_density.clone(),
            input_assignment.clone(),
            &mut multiexp_kern,
        )
    };

    let b_g1_aux = if !check_for_higher_prio!() || keep_cpu {
        #[cfg(feature = "gpu")]
        {
            info!("Multiexp 6 Prover found acquire lock, switching to CPU");
            // Free the incoming process to use the GPU
            if !keep_cpu {
                keep_cpu = true;
                lock.unlock()?;
            }
        }
        multiexp(
            &worker,
            b_g1_aux_source,
            b_aux_density.clone(),
            aux_assignment.clone(),
            &mut None,
        )
    } else {
        info!("Multiexp 6 Prover NO acquire lock, keeping GPU");
        multiexp(
            &worker,
            b_g1_aux_source,
            b_aux_density.clone(),
            aux_assignment.clone(),
            &mut multiexp_kern,
        )
    };

    let (b_g2_inputs_source, b_g2_aux_source) =
        params.get_b_g2(b_input_density_total, b_aux_density_total)?;

    let b_g2_inputs = if !check_for_higher_prio!() || keep_cpu {
        #[cfg(feature = "gpu")]
        {
            info!("Multiexp 7 Prover found acquire lock, switching to CPU");
            // Free the incoming process to use the GPU
            if !keep_cpu {
                keep_cpu = true;
                lock.unlock()?;
            }
        }
        multiexp(
            &worker,
            b_g2_inputs_source,
            b_input_density,
            input_assignment,
            &mut None,
        )
    } else {
        info!("Multiexp 7 Prover NO acquire lock, keeping GPU");
        multiexp(
            &worker,
            b_g2_inputs_source,
            b_input_density,
            input_assignment,
            &mut multiexp_kern,
        )
    };

    let b_g2_aux = if !check_for_higher_prio!() || keep_cpu {
        #[cfg(feature = "gpu")]
        {
            info!("Multiexp 8 Prover found acquire lock, switching to CPU");
            // Free the incoming process to use the GPU
            if !keep_cpu {
                lock.unlock()?;
            }
        }
        multiexp(
            &worker,
            b_g2_aux_source,
            b_aux_density,
            aux_assignment,
            &mut None,
        )
    } else {
        info!("Multiexp 8 Prover NO acquire lock, keeping GPU");
        multiexp(
            &worker,
            b_g2_aux_source,
            b_aux_density,
            aux_assignment,
            &mut multiexp_kern,
        )
    };
    #[cfg(feature = "gpu")]
    gpu::unlock(lock);

    if vk.delta_g1.is_zero() || vk.delta_g2.is_zero() {
        // If this element is zero, someone is trying to perform a
        // subversion-CRS attack.
        return Err(SynthesisError::UnexpectedIdentity);
    }

    let mut g_a = vk.delta_g1.mul(r);
    g_a.add_assign_mixed(&vk.alpha_g1);
    let mut g_b = vk.delta_g2.mul(s);
    g_b.add_assign_mixed(&vk.beta_g2);
    let mut g_c;
    {
        let mut rs = r;
        rs.mul_assign(&s);

        g_c = vk.delta_g1.mul(rs);
        g_c.add_assign(&vk.alpha_g1.mul(s));
        g_c.add_assign(&vk.beta_g1.mul(r));
    }
    let mut a_answer = a_inputs.wait()?;
    a_answer.add_assign(&a_aux.wait()?);
    g_a.add_assign(&a_answer);
    a_answer.mul_assign(s);
    g_c.add_assign(&a_answer);

    let mut b1_answer = b_g1_inputs.wait()?;
    b1_answer.add_assign(&b_g1_aux.wait()?);
    let mut b2_answer = b_g2_inputs.wait()?;
    b2_answer.add_assign(&b_g2_aux.wait()?);

    g_b.add_assign(&b2_answer);
    b1_answer.mul_assign(r);
    g_c.add_assign(&b1_answer);
    g_c.add_assign(&h.wait()?);
    g_c.add_assign(&l.wait()?);

    Ok(Proof {
        a: g_a.into_affine(),
        b: g_b.into_affine(),
        c: g_c.into_affine(),
    })
}
