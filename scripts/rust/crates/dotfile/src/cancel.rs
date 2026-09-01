use std::sync::atomic::{AtomicBool, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);

pub fn reset() {
    CANCELLED.store(false, Ordering::Release);
}

pub fn request() {
    CANCELLED.store(true, Ordering::Release);
}

pub fn requested() -> bool {
    CANCELLED.load(Ordering::Acquire)
}

pub fn check() -> Result<(), String> {
    if requested() {
        Err("cancelled".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}
