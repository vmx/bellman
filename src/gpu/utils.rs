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
            ("TITAN RTX".to_string(), 4608),

            ("Tesla V100".to_string(), 5120),
            ("Tesla P100".to_string(), 3584),

            ("GeForce RTX 2080 Ti".to_string(), 4352),
            ("GeForce RTX 2080 SUPER".to_string(), 3072),
            ("GeForce RTX 2080".to_string(), 2944),
            ("GeForce RTX 2070 SUPER".to_string(), 2560),

            ("GeForce GTX 1080 Ti".to_string(), 3584),
            ("GeForce GTX 1080".to_string(), 2560),
            ("GeForce GTX 2060".to_string(), 1920),
            ("GeForce GTX 1660 Ti".to_string(), 1536),
            ("GeForce GTX 1060".to_string(), 1280),
            ("GeForce GTX 1650 SUPER".to_string(), 1280),
            ("GeForce GTX 1650".to_string(), 896),
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

#[derive(Debug)]
pub struct LockedFile(File);

pub fn get_lock_file() -> io::Result<LockedFile> {
    info!("Creating GPU lock file");
    let file = File::create(LOCK_NAME)?;

    file.lock_exclusive()?;

    info!("GPU lock file acquired");
    Ok(LockedFile(file))
}

pub fn unlock(lock: &LockedFile) -> io::Result<()> {
    lock.0.unlock()?;
    info!("GPU lock file released");
    Ok(())
}

//-----

const GPU_LOCK_NAME: &str = "/tmp/bellman.gpu.lock";

#[derive(Debug)]
pub struct GPULock(File);
impl GPULock {
    pub fn new() -> io::Result<GPULock> {
        let file = File::create(GPU_LOCK_NAME)?;
        Ok(GPULock(file))
    }
    pub fn lock(&mut self) -> io::Result<()> {
        info!("Acquiring GPU lock...");
        self.0.lock_exclusive()?;
        info!("GPU lock acquired!");
        Ok(())
    }
    pub fn unlock(&mut self) -> io::Result<()> {
        self.0.unlock()?;
        info!("GPU lock released!");
        Ok(())
    }
}

pub fn gpu_is_available() -> Result<bool, io::Error> {
    let file = File::create(GPU_LOCK_NAME)?;
    let _test = file.try_lock_exclusive()?;
    drop(file);
    Ok(true)
}

const PRIORITY_LOCK_NAME: &str = "/tmp/bellman.priority.lock";

use std::cell::RefCell;
thread_local!(static IS_ME: RefCell<bool> = RefCell::new(false));

#[derive(Debug)]
pub struct PriorityLock(File);
impl PriorityLock {
    pub fn new() -> io::Result<PriorityLock> {
        let file = File::create(PRIORITY_LOCK_NAME)?;
        Ok(PriorityLock(file))
    }
    pub fn lock(&mut self) -> io::Result<()> {
        IS_ME.with(|f| *f.borrow_mut() = true);
        info!("Acquiring priority lock...");
        self.0.lock_exclusive()?;
        info!("Priority lock acquired!");
        Ok(())
    }
    pub fn unlock(&mut self) -> io::Result<()> {
        IS_ME.with(|f| *f.borrow_mut() = false);
        self.0.unlock()?;
        info!("Priority lock released!");
        Ok(())
    }
    pub fn can_lock() -> io::Result<bool> {
        // Either taken by me or not taken by somebody else
        let is_me = IS_ME.with(|f| *f.borrow());
        Ok(is_me
            || File::create(PRIORITY_LOCK_NAME)?
                .try_lock_exclusive()
                .is_ok())
    }
}
