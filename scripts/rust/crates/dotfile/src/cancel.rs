use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);
static CHILD_SIGNAL: AtomicI32 = AtomicI32::new(0);

pub fn flag() -> &'static AtomicBool {
    &CANCELLED
}

pub fn reset() {
    CANCELLED.store(false, Ordering::Release);
    CHILD_SIGNAL.store(0, Ordering::Release);
}

pub fn request() {
    CANCELLED.store(true, Ordering::Release);
}

pub fn request_signal(signal: i32) {
    CHILD_SIGNAL.store(signal, Ordering::Release);
    request();
}

pub fn signal() -> i32 {
    let signal = workstation::screen::termination_signal();
    if signal != 0 {
        signal
    } else {
        CHILD_SIGNAL.load(Ordering::Acquire)
    }
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
