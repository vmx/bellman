extern crate bellperson;
extern crate ff;
extern crate log;
extern crate paired;
extern crate rand;

use bellperson::{
    gpu::PriorityLock,
    groth16::{
        create_random_proof, generate_random_parameters, prepare_verifying_key, verify_proof,
    },
    Circuit, ConstraintSystem, SynthesisError,
};
use ff::{Field, PrimeField};
use log::info;
use paired::{bls12_381::Bls12, Engine};
use rand::thread_rng;

use std::thread;
use std::time::Duration;
use std::process;

#[derive(Clone)]
pub struct DummyDemo<E: Engine> {
    pub xx: Option<E::Fr>,
    pub interations: u64,
}

impl<E: Engine> Circuit<E> for DummyDemo<E> {
    fn synthesize<CS: ConstraintSystem<E>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        let mut x_val = E::Fr::from_str("2");
        let mut x = cs.alloc(|| "", || x_val.ok_or(SynthesisError::AssignmentMissing))?;

        for _ in 0..self.interations {
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

pub fn colored_with_thread(
    writer: &mut dyn std::io::Write,
    now: &mut flexi_logger::DeferredNow,
    record: &flexi_logger::Record,
) -> Result<(), std::io::Error> {
    let level = record.level();
    write!(
        writer,
        "{} {} {} {} {} > {}",
        now.now().format("%Y-%m-%dT%H:%M:%S%.3f"),
        process::id(),
        thread::current()
            .name()
            .unwrap_or(&format!("{:?}", thread::current().id())),
        flexi_logger::style(level, level),
        record.module_path().unwrap_or("<unnamed>"),
        record.args(),
    )
}

#[cfg(feature = "gpu-test")]
#[test]
pub fn test_parallel_prover() {
    flexi_logger::Logger::with_env()
        .format(colored_with_thread)
        .start()
        .expect("Initializing logger failed. Was another logger already initialized?");
    let rng = &mut thread_rng();

    println!("Initializing circuit...");
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

    let lower_thread = thread::spawn(move || {
        info!("Creating proof from LOWER priority process...");
        let rng = &mut thread_rng();
        let proof_lower = create_random_proof(c2, &params2, rng).unwrap();
        let result = verify_proof(&pvk2, &proof_lower, &[]).unwrap();
        info!("Proof Lower is verified: {}", result);
    });

    // Have higher prio proof wait long enough to interrupt lower
    thread::sleep(Duration::from_millis(2000));

    {
        info!("Creating proof from HIGHER priority process...");
        let _lock = PriorityLock::lock();
        let proof_higher = create_random_proof(c, &params, rng).unwrap();
        let result = verify_proof(&pvk, &proof_higher, &[]).unwrap();
        info!("Proof Higher is verified: {}", result);
    }

    lower_thread.join().unwrap();
}
