use fs2::FileExt;
use log::info;
use std::fs::File;
use std::io;

pub const LOCK_NAME: &str = "/tmp/bellman.lock";
pub const ACQUIRE_NAME: &str = "/tmp/acquire_bellman.lock";

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
    pub fn gpu_is_available() -> Result<bool, io::Error> {
        let file = File::create(GPU_LOCK_NAME)?;
        let _test = file.try_lock_exclusive()?;
        drop(file);
        Ok(true)
    }
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
