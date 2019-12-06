#![allow(unused_imports)]
#![allow(unused_variables)]
extern crate bellperson;
extern crate paired;
extern crate rand;
extern crate ff;
extern crate log;
use bellperson::{Circuit, ConstraintSystem, SynthesisError};
use bellperson::groth16::{Parameters};
use paired::{Engine};
use ff::{Field, PrimeField};

use std::fs::File;
use std::io::prelude::*;

use std::time::{Duration, Instant};
use std::thread;

// For randomness (during paramgen and proof generation)
use self::rand::{thread_rng, Rng};

// Bring in some tools for using pairing-friendly curves
// use self::paired::{
//     Engine
// };

//use self::ff::{Field,PrimeField};

// We're going to use the BLS12-381 pairing-friendly elliptic curve.
use self::paired::bls12_381::{
    Bls12,
    Fr
};

// We'll use these interfaces to construct our circuit.
// use self::bellperson::{
//     Circuit,
//     ConstraintSystem,
//     SynthesisError
// };

// We're going to use the Groth16 proving system.
use self::bellperson::groth16::{
    Proof,
    generate_random_parameters,
    prepare_verifying_key,
    create_random_proof,
    verify_proof,
};

#[derive(Clone)]
pub struct DummyDemo<E: Engine> {
    pub xx: Option<E::Fr>,
}

impl <E: Engine> Circuit<E> for DummyDemo<E> {
    fn synthesize<CS: ConstraintSystem<E>>(
        self,
        cs: &mut CS
    ) -> Result<(), SynthesisError>
    {

        let mut x_val = E::Fr::from_str("2");
        let mut x = cs.alloc(|| "", || {
            x_val.ok_or(SynthesisError::AssignmentMissing)
        })?;

        for k in 0..1_000 {
            // Allocate: x * x = x2
            let x2_val = x_val.map(|mut e| {
                e.square();
                e
            });
            let x2 = cs.alloc(|| "", || {
                x2_val.ok_or(SynthesisError::AssignmentMissing)
            })?;

            // Enforce: x * x = x2
            cs.enforce(
                || "",
                |lc| lc + x,
                |lc| lc + x,
                |lc| lc + x2
            );

            x = x2;
            x_val = x2_val;
        }

        cs.enforce(
            || "",
            |lc| lc + (x_val.unwrap(), CS::one()),
            |lc| lc + CS::one(),
            |lc| lc + x
        );

        Ok(())
    }
}


//mod dummy;

#[cfg(feature = "gpu-test")]
#[test]
pub fn test_parallel_prover(){
    env_logger::init();
    use paired::bls12_381::{Bls12, Fr};
    use rand::thread_rng;
    use bellperson::groth16::{
        create_proof, create_random_proof, generate_random_parameters, prepare_verifying_key, verify_proof, Proof,
    };

    println!("Initializing circuit...");

    let rng = &mut thread_rng();

    println!("Creating parameters...");

    let c = DummyDemo::<Bls12> {
        xx: None
    };

    let params = generate_random_parameters(c.clone(), rng).unwrap();
    let params2 = generate_random_parameters(c.clone(), rng).unwrap();

    // Prepare the verification key (for proof verification)
    let pvk = prepare_verifying_key(&params.vk);
    let pvk2 = prepare_verifying_key(&params2.vk);

    //let now = Instant::now();

    // Create an instance of circuit
    let c = DummyDemo::<Bls12> {
        xx: Fr::from_str("3")
    };

    let c2 = DummyDemo::<Bls12> {
        xx: Fr::from_str("3")
    };

    // generate randomness
    let r1 = Fr::random(rng);
    let s1 = Fr::random(rng);
    let r2 = Fr::random(rng);
    let s2 = Fr::random(rng);

    thread::spawn(move || {
        println!("Creating proof from HIGHER priority process...");
        // Create an instance of circuit

        thread::sleep(Duration::from_millis(1));
        let proof_higher = create_proof(c2, &params2, r2, s2).unwrap();
        println!("Proof Higher is verified: {}", verify_proof(
            &pvk2,
            &proof_higher,
            &[]
        ).unwrap());
    });

    println!("Creating proof from LOWER priority process...");
    // Create a groth16 proof with our parameters.
    thread::sleep(Duration::from_millis(1000));
    let proof_lower = create_proof(c, &params, r1, s1).unwrap();
    //thread::sleep(Duration::from_millis(1000));
    //println!("Total proof gen finished in {}s and {}ms", now.elapsed().as_secs(), now.elapsed().subsec_nanos()/1000000);

    println!("Proof Lower is verified: {}", verify_proof(
        &pvk,
        &proof_lower,
        &[]
    ).unwrap());
}

