use fs2::FileExt;
use log::info;
use std::fs::File;
use std::io;

const GPU_LOCK_NAME: &str = "/tmp/bellman.gpu.lock";

#[derive(Debug)]
pub struct GPULock(File);
impl GPULock {
    pub fn new() -> GPULock {
        GPULock(File::create(GPU_LOCK_NAME).unwrap())
    }
    pub fn lock(&mut self) {
        info!("Acquiring GPU lock...");
        self.0.lock_exclusive().unwrap();
        info!("GPU lock acquired!");
    }
    pub fn unlock(&mut self) {
        self.0.unlock().unwrap();
        info!("GPU lock released!");
    }
    pub fn gpu_is_available() -> bool {
        File::create(GPU_LOCK_NAME)
            .unwrap()
            .try_lock_exclusive()
            .is_ok()
    }
}

const PRIORITY_LOCK_NAME: &str = "/tmp/bellman.priority.lock";

use std::cell::RefCell;
thread_local!(static IS_ME: RefCell<bool> = RefCell::new(false));

#[derive(Debug)]
pub struct PriorityLock(File);
impl PriorityLock {
    pub fn new() -> PriorityLock {
        PriorityLock(File::create(PRIORITY_LOCK_NAME).unwrap())
    }
    pub fn lock(&mut self) {
        IS_ME.with(|f| *f.borrow_mut() = true);
        info!("Acquiring priority lock...");
        self.0.lock_exclusive().unwrap();
        info!("Priority lock acquired!");
    }
    pub fn unlock(&mut self) {
        IS_ME.with(|f| *f.borrow_mut() = false);
        self.0.unlock().unwrap();
        info!("Priority lock released!");
    }
    pub fn can_lock() -> bool {
        // Either taken by me or not taken by somebody else
        let is_me = IS_ME.with(|f| *f.borrow());
        is_me
            || File::create(PRIORITY_LOCK_NAME)
                .unwrap()
                .try_lock_exclusive()
                .is_ok()
    }
}
