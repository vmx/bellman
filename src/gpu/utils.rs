use crate::gpu::error::{GPUError, GPUResult};
use ocl::{Device, Platform};

use fs2::FileExt;
use log::info;
use std::collections::HashMap;
use std::fs::File;
use std::{env, io};

pub const GPU_NVIDIA_PLATFORM_NAME: &str = "NVIDIA CUDA";
// pub const CPU_INTEL_PLATFORM_NAME: &str = "Intel(R) CPU Runtime for OpenCL(TM) Applications";

pub fn get_devices(platform_name: &str) -> GPUResult<Vec<Device>> {
    if env::var("BELLMAN_NO_GPU").is_ok() {
        return Err(GPUError {
            msg: "GPU accelerator is disabled!".to_string(),
        });
    }

    let platform = Platform::list()?.into_iter().find(|&p| match p.name() {
        Ok(p) => p == platform_name,
        Err(_) => false,
    });
    match platform {
        Some(p) => Ok(Device::list_all(p)?),
        None => Err(GPUError {
            msg: "GPU platform not found!".to_string(),
        }),
    }
}

lazy_static::lazy_static! {
    static ref CORE_COUNTS: HashMap<String, usize> = {
        let mut core_counts : HashMap<String, usize> = vec![
            ("GeForce RTX 2080 Ti".to_string(), 4352),
            ("GeForce RTX 2080 SUPER".to_string(), 3072),
            ("GeForce RTX 2080".to_string(), 2944),
            ("GeForce GTX 1080 Ti".to_string(), 3584),
            ("GeForce GTX 1080".to_string(), 2560),
            ("GeForce GTX 1060".to_string(), 1280),
        ].into_iter().collect();

        match env::var("BELLMAN_CUSTOM_GPU").and_then(|var| {
            for card in var.split(",") {
                let splitted = card.split(":").collect::<Vec<_>>();
                if splitted.len() != 2 { panic!("Invalid BELLMAN_CUSTOM_GPU!"); }
                let name = splitted[0].trim().to_string();
                let cores : usize = splitted[1].trim().parse().expect("Invalid BELLMAN_CUSTOM_GPU!");
                info!("Adding \"{}\" to GPU list with {} CUDA cores.", name, cores);
                core_counts.insert(name, cores);
            }
            Ok(())
        }) { Err(_) => { }, Ok(_) => { } }

        core_counts
    };
}

pub fn get_core_count(d: Device) -> GPUResult<usize> {
    match CORE_COUNTS.get(&d.name()?[..]) {
        Some(&cores) => Ok(cores),
        None => Err(GPUError {
            msg: "Device unknown!".to_string(),
        }),
    }
}

pub fn get_memory(d: Device) -> GPUResult<u64> {
    match d.info(ocl::enums::DeviceInfo::GlobalMemSize)? {
        ocl::enums::DeviceInfoResult::GlobalMemSize(sz) => Ok(sz),
        _ => Err(GPUError {
            msg: "Cannot extract GPU memory!".to_string(),
        }),
    }
}

pub const LOCK_NAME: &str = "/tmp/bellman.lock";
pub const ACQUIRE_NAME: &str = "/tmp/acquire_bellman.lock";
pub const LOCK_NULL: &str = "/tmp/null.lock";

pub struct LFile(File);

pub fn get_lock_file() -> io::Result<File> {
    info!("Creating GPU lock file");
    let file = File::create(LOCK_NAME)?;

    file.lock_exclusive()?;

    info!("GPU lock file acquired");
    Ok(file)
}

pub fn pseudo_lock() -> io::Result<File> {
    let file = File::create(LOCK_NULL)?;
    Ok(file)
}

pub fn unlock(lock: File) {
    drop(lock);
    info!("GPU lock file released");
}

pub fn gpu_is_available() -> Result<bool, io::Error> {
    let file = File::create(LOCK_NAME)?;
    let _test = file.try_lock_exclusive()?;
    drop(file);
    Ok(true)
}

pub fn acquire_gpu() -> io::Result<File> {
    info!("Creating Acquire GPU lock file");
    let file = File::create(ACQUIRE_NAME)?;

    file.lock_exclusive()?;

    info!("Higher Priority GPU lock file acquired");
    Ok(file)
}

pub fn gpu_is_not_acquired() -> Result<bool, io::Error> {
    let file = File::create(ACQUIRE_NAME)?;
    let _test = file.try_lock_exclusive()?;
    drop(file);
    Ok(true)
}

pub fn drop_acquire_lock(acquire_lock: File) {
    drop(acquire_lock);
    info!("GPU acquire lock file released");
}
