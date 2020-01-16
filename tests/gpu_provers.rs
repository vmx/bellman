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
    pub interations: u64,
}

impl<E: Engine> Circuit<E> for DummyDemo<E> {
    fn synthesize<CS: ConstraintSystem<E>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        let mut x_val = E::Fr::from_str("2");
        let mut x = cs.alloc(|| "", || x_val.ok_or(SynthesisError::AssignmentMissing))?;

        for k in 0..self.interations {
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
    use bellperson::gpu::{GPULock, PriorityLock};
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

    // Higher prio circuit
    let c = DummyDemo::<Bls12> {
        xx: None,
        interations: 10_000,
    };
    // Lower prio circuit
    let c2 = DummyDemo::<Bls12> {
        xx: None,
        interations: 500_000,
    };

    let params = generate_random_parameters(c.clone(), rng).unwrap();
    let params2 = generate_random_parameters(c2.clone(), rng).unwrap();

    // Prepare the verification key (for proof verification)
    let pvk = prepare_verifying_key(&params.vk);
    let pvk2 = prepare_verifying_key(&params2.vk);

    // generate randomness
    let r1 = Fr::random(rng);
    let s1 = Fr::random(rng);
    let r2 = Fr::random(rng);
    let s2 = Fr::random(rng);

    let lower_thread = thread::spawn(move || {
        info!("Creating proof from LOWER priority process...");
        // Create an instance of circuit
        let proof_lower = create_proof(c2, &params2, r2, s2).unwrap();
        info!(
            "Proof Lower is verified: {}",
            verify_proof(&pvk2, &proof_lower, &[]).unwrap()
        );
    });

    // Have higher prio proof wait long enough to interupt lower
    thread::sleep(Duration::from_millis(2000));
    info!("Creating proof from HIGHER priority process...");
    let mut prio_lock = PriorityLock::new();
    prio_lock.lock();
    let proof_higher = create_proof(c, &params, r1, s1).unwrap();
    info!("Higher Process proof finished, releasing priority lock...");
    drop(prio_lock);

    //println!("Total proof gen finished in {}s and {}ms", now.elapsed().as_secs(), now.elapsed().subsec_nanos()/1000000);
    info!(
        "Proof Higher is verified: {}",
        verify_proof(&pvk, &proof_higher, &[]).unwrap()
    );

    lower_thread.join().unwrap();
}
