#![allow(unused_imports)]
#![allow(unused_variables)]
extern crate bellperson;
extern crate ff;
extern crate log;
extern crate paired;
extern crate rand;
use bellperson::gpu;
use bellperson::groth16::Parameters;
use bellperson::{Circuit, ConstraintSystem, SynthesisError};
use log::info;

use ff::{Field, PrimeField};
use paired::Engine;

use std::fs::File;
use std::io::prelude::*;

use std::thread;
use std::time::{Duration, Instant};
use std::{env, io};

// For randomness (during paramgen and proof generation)
use self::rand::{thread_rng, Rng};

// We're going to use the BLS12-381 pairing-friendly elliptic curve.
use self::paired::bls12_381::{Bls12, Fr};

// We're going to use the Groth16 proving system.
use self::bellperson::groth16::{
    create_random_proof, generate_random_parameters, prepare_verifying_key, verify_proof, Proof,
};

#[derive(Clone)]
pub struct DummyDemo<E: Engine> {
    pub xx: Option<E::Fr>,
}

impl<E: Engine> Circuit<E> for DummyDemo<E> {
    fn synthesize<CS: ConstraintSystem<E>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        let mut x_val = E::Fr::from_str("2");
        let mut x = cs.alloc(|| "", || x_val.ok_or(SynthesisError::AssignmentMissing))?;

        for k in 0..500_000 {
            // Allocate: x * x = x2
            let x2_val = x_val.map(|mut e| {
                e.square();
                e
            });
            let x2 = cs.alloc(|| "", || x2_val.ok_or(SynthesisError::AssignmentMissing))?;

            // Enforce: x * x = x2
            cs.enforce(|| "", |lc| lc + x, |lc| lc + x, |lc| lc + x2);

            x = x2;
            x_val = x2_val;
        }

        cs.enforce(
            || "",
            |lc| lc + (x_val.unwrap(), CS::one()),
            |lc| lc + CS::one(),
            |lc| lc + x,
        );

        Ok(())
    }
}

#[cfg(feature = "gpu-test")]
#[test]
pub fn test_parallel_prover() {
    env_logger::init();
    use bellperson::groth16::{
        create_proof, create_random_proof, generate_random_parameters, prepare_verifying_key,
        verify_proof, Proof,
    };
    use paired::bls12_381::{Bls12, Fr};
    use rand::thread_rng;

    println!("Initializing circuit...");

    let rng = &mut thread_rng();

    println!("Creating parameters...");

    let c = DummyDemo::<Bls12> { xx: None };

    let params = generate_random_parameters(c.clone(), rng).unwrap();
    let params2 = generate_random_parameters(c.clone(), rng).unwrap();

    // Prepare the verification key (for proof verification)
    let pvk = prepare_verifying_key(&params.vk);
    let pvk2 = prepare_verifying_key(&params2.vk);

    //let now = Instant::now();

    // Create an instance of circuit
    let c = DummyDemo::<Bls12> {
        xx: Fr::from_str("3"),
    };

    let c2 = DummyDemo::<Bls12> {
        xx: Fr::from_str("3"),
    };

    // generate randomness
    let r1 = Fr::random(rng);
    let s1 = Fr::random(rng);
    let r2 = Fr::random(rng);
    let s2 = Fr::random(rng);

    // test function to see if GPU is available
    let res = match gpu::gpu_is_available() {
        Ok(n) => n,
        Err(err) => false,
    };

    if res == true {
        info!("GPU is available!...");
    }

    thread::spawn(move || {
        info!("Creating proof from LOWER priority process...");
        // Create an instance of circuit
        let proof_lower = create_proof(c2, &params2, r2, s2).unwrap();
        info!(
            "Proof Lower is verified: {}",
            verify_proof(&pvk2, &proof_lower, &[]).unwrap()
        );
    });

    // Have higher prio proof wait long enough to interupt lower
    thread::sleep(Duration::from_millis(3100));
    info!("Creating proof from HIGHER priority process...");

    let check = match gpu::gpu_is_available() {
        Ok(n) => n,
        Err(err) => false,
    };

    if check != true {
        info!("GPU is NOT Available! Attempting to acuire the GPU...");
        let a_lock = Some(gpu::acquire_gpu().unwrap());

        // We need to drop the acquire lock as soon as the lower prio
        // process has freed the main lock so that the higher uses GPU
        loop {
            //info!("checking to see if lower prio process has freed GPU");
            // let available = match gpu::gpu_is_available() {
            //     Ok(n) => n,
            //     Err(err) => false,
            // };
            if gpu::gpu_is_available().unwrap_or(false) {
                info!("GPU free from lower prio process. Dropping acquire gpu file lock from switching process...");
                //gpu::drop_acquire_lock(a_lock.unwrap());
                gpu::priority_unlock(a_lock.unwrap());
                break;
            };
            continue;
        }
    };

    let proof_higher = create_proof(c, &params, r1, s1).unwrap();

    //println!("Total proof gen finished in {}s and {}ms", now.elapsed().as_secs(), now.elapsed().subsec_nanos()/1000000);
    info!(
        "Proof Higher is verified: {}",
        verify_proof(&pvk, &proof_higher, &[]).unwrap()
    );
    // Let lower prior proof finish
    thread::sleep(Duration::from_millis(4100));
}
