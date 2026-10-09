#![allow(unsafe_code)]

use std::ffi::c_int;

const USER_GENERATED: u32 = 0x200;
const EVENT_RECORD_LEN: usize = 0xf8;

#[repr(C)]
#[derive(Default)]
struct ProcessSerialNumber {
    high: u32,
    low: u32,
}

#[link(name = "SkyLight", kind = "framework")]
unsafe extern "C" {
    fn SLSMainConnectionID() -> c_int;
    fn SLSGetWindowOwner(connection: c_int, window: u32, owner: *mut c_int) -> i32;
    fn SLSGetConnectionPSN(connection: c_int, psn: *mut ProcessSerialNumber) -> i32;
    fn _SLPSSetFrontProcessWithOptions(
        psn: *mut ProcessSerialNumber,
        window: u32,
        mode: u32,
    ) -> i32;
    fn SLPSPostEventRecordTo(psn: *mut ProcessSerialNumber, bytes: *mut u8) -> i32;
}

pub fn focus(window: u32) -> Result<(), String> {
    let mut psn = owner(window).ok_or("window not found")?;
    // SAFETY: psn is a live local resolved by the window server for this window.
    let fronted = unsafe { _SLPSSetFrontProcessWithOptions(&mut psn, window, USER_GENERATED) };
    check(fronted, "front process")?;
    make_key(&mut psn, window)
}

fn owner(window: u32) -> Option<ProcessSerialNumber> {
    let mut connection = 0;
    let mut psn = ProcessSerialNumber::default();
    // SAFETY: both out-pointers reference live locals of the declared types.
    let found = unsafe {
        SLSGetWindowOwner(SLSMainConnectionID(), window, &mut connection) == 0
            && SLSGetConnectionPSN(connection, &mut psn) == 0
    };
    found.then_some(psn)
}

// Synthetic mouse down/up records, laid out as yabai's window_manager_make_key_window.
fn make_key(psn: &mut ProcessSerialNumber, window: u32) -> Result<(), String> {
    let mut record = [0u8; EVENT_RECORD_LEN];
    record[0x04] = 0xf8;
    record[0x3a] = 0x10;
    record[0x3c..0x40].copy_from_slice(&window.to_ne_bytes());
    record[0x20..0x30].fill(0xff);
    for phase in [0x01, 0x02] {
        record[0x08] = phase;
        // SAFETY: record is a writable buffer of the EVENT_RECORD_LEN bytes the call reads.
        let posted = unsafe { SLPSPostEventRecordTo(psn, record.as_mut_ptr()) };
        check(posted, "make key")?;
    }
    Ok(())
}

fn check(code: i32, step: &str) -> Result<(), String> {
    match code {
        0 => Ok(()),
        code => Err(format!("{step}: error {code}")),
    }
}
