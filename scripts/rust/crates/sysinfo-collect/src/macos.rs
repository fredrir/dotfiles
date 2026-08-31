use std::ffi::{CString, c_void};
use std::fs;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType, kCFAllocatorDefault};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use io_kit_sys::ret::kIOReturnSuccess;
use io_kit_sys::types::{io_iterator_t, io_object_t};
use io_kit_sys::{
    IOIteratorNext, IOObjectConformsTo, IOObjectRelease, IORegistryEntryCreateCFProperty,
    IORegistryEntryGetName, IORegistryEntrySearchCFProperty, IOServiceGetMatchingServices,
    IOServiceMatching, kIOMasterPortDefault, kIORegistryIterateParents,
    kIORegistryIterateRecursively,
};
use serde_json::{Value, json};

use crate::Module;

pub fn collect(out: &mut Vec<Module>) {
    out.push(("OS", os_module()));
    out.push(("Kernel", kernel_module()));
    out.push(("CPU", cpu_module()));
    out.push(("GPU", gpu_module()));
    out.push((
        "PhysicalMemory",
        json!([{
            "size": sysctl_u64("hw.memsize").unwrap_or(0),
            "installed": true,
            "type": "",
            "vendor": "",
        }]),
    ));
    out.push(("PhysicalDisk", physical_disks()));
    out.push((
        "Board",
        json!({
            "name": sysctl_string("hw.target").unwrap_or_default(),
            "vendor": "Apple Inc.",
            "version": "",
        }),
    ));
    let (batteries, adapters) = power_modules();
    out.push(("Battery", batteries));
    out.push(("PowerAdapter", adapters));
    out.push((
        "WM",
        json!({
            "prettyName": "Quartz Compositor",
            "processName": "WindowServer",
            "protocolName": "",
        }),
    ));
}

// --- sysctl ------------------------------------------------------------------

fn sysctl_string(name: &str) -> Option<String> {
    let name = CString::new(name).ok()?;
    let mut size = 0usize;
    let found = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if found != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    let found = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if found != 0 {
        return None;
    }
    buffer.truncate(size.saturating_sub(1));
    String::from_utf8(buffer).ok()
}

fn sysctl_u64(name: &str) -> Option<u64> {
    let name = CString::new(name).ok()?;
    let mut value = 0u64;
    let mut size = std::mem::size_of::<u64>();
    let found = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (found == 0).then_some(value)
}

fn arm_feature(name: &str) -> bool {
    sysctl_u64(&format!("hw.optional.arm.{name}")) == Some(1)
}

// --- IOKit helpers -----------------------------------------------------------

fn matching_services(class: &str) -> Vec<io_object_t> {
    let Ok(class) = CString::new(class) else {
        return Vec::new();
    };
    let matching = unsafe { IOServiceMatching(class.as_ptr()) };
    if matching.is_null() {
        return Vec::new();
    }
    let mut iterator: io_iterator_t = 0;
    let status =
        unsafe { IOServiceGetMatchingServices(kIOMasterPortDefault, matching, &mut iterator) };
    if status != kIOReturnSuccess {
        return Vec::new();
    }
    let mut services = Vec::new();
    loop {
        let service = unsafe { IOIteratorNext(iterator) };
        if service == 0 {
            break;
        }
        services.push(service);
    }
    unsafe { IOObjectRelease(iterator) };
    services
}

fn release(services: Vec<io_object_t>) {
    for service in services {
        unsafe { IOObjectRelease(service) };
    }
}

fn registry_property(entry: io_object_t, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    let value = unsafe {
        IORegistryEntryCreateCFProperty(entry, key.as_concrete_TypeRef(), kCFAllocatorDefault, 0)
    };
    if value.is_null() {
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

fn search_property(entry: io_object_t, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    let plane = CString::new("IOService").ok()?;
    let value = unsafe {
        IORegistryEntrySearchCFProperty(
            entry,
            plane.as_ptr(),
            key.as_concrete_TypeRef(),
            kCFAllocatorDefault,
            kIORegistryIterateRecursively | kIORegistryIterateParents,
        )
    };
    if value.is_null() {
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

fn as_string(value: Option<CFType>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.downcast::<CFString>() {
        return Some(text.to_string());
    }
    // Device-tree strings arrive as NUL-terminated CFData.
    let data = value.downcast::<CFData>()?;
    let bytes: Vec<u8> = data
        .bytes()
        .iter()
        .copied()
        .take_while(|byte| *byte != 0)
        .collect();
    String::from_utf8(bytes).ok()
}

fn as_i64(value: Option<CFType>) -> Option<i64> {
    value?.downcast::<CFNumber>()?.to_i64()
}

fn as_f64(value: Option<CFType>) -> Option<f64> {
    value?.downcast::<CFNumber>()?.to_f64()
}

fn as_bool(value: Option<CFType>) -> Option<bool> {
    Some(value?.downcast::<CFBoolean>()?.into())
}

fn as_bytes(value: Option<CFType>) -> Option<Vec<u8>> {
    Some(value?.downcast::<CFData>()?.bytes().to_vec())
}

fn dict_value(dict: &CFDictionary, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    let raw = dict.find(key.as_concrete_TypeRef() as *const c_void)?;
    Some(unsafe { CFType::wrap_under_get_rule(*raw as CFTypeRef) })
}

fn entry_name(entry: io_object_t) -> String {
    let mut buffer = [0i8; 128];
    if unsafe { IORegistryEntryGetName(entry, buffer.as_mut_ptr()) } != kIOReturnSuccess {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .to_string()
}

// --- OS / Kernel -------------------------------------------------------------

fn plist_value(body: &str, key: &str) -> Option<String> {
    let position = body.find(&format!("<key>{key}</key>"))?;
    let rest = &body[position..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest.find("</string>")?;
    Some(rest[start..end].to_string())
}

fn codename(version: &str) -> &'static str {
    match version.split('.').next().unwrap_or_default() {
        "26" => "Tahoe",
        "15" => "Sequoia",
        "14" => "Sonoma",
        "13" => "Ventura",
        "12" => "Monterey",
        "11" => "Big Sur",
        _ => "",
    }
}

fn os_module() -> Value {
    let body =
        fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").unwrap_or_default();
    let version = plist_value(&body, "ProductVersion").unwrap_or_default();
    let build = plist_value(&body, "ProductBuildVersion").unwrap_or_default();
    let name = codename(&version);
    let mut pretty = "macOS".to_string();
    if !name.is_empty() {
        pretty = format!("macOS {name}");
    }
    if !version.is_empty() {
        pretty = format!("{pretty} {version}");
    }
    if !build.is_empty() {
        pretty = format!("{pretty} ({build})");
    }
    json!({
        "id": "macos",
        "name": "macOS",
        "codename": name,
        "buildID": build,
        "prettyName": pretty,
        "version": version,
        "versionID": version,
    })
}

fn kernel_module() -> Value {
    let mut names: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut names) } != 0 {
        return json!({});
    }
    let field = |bytes: &[libc::c_char]| {
        unsafe { std::ffi::CStr::from_ptr(bytes.as_ptr()) }
            .to_string_lossy()
            .to_string()
    };
    json!({
        "name": field(&names.sysname),
        "release": field(&names.release),
        "version": field(&names.version),
        "architecture": field(&names.machine),
    })
}

// --- CPU ---------------------------------------------------------------------

fn max_frequency_mhz() -> u64 {
    let services = matching_services("AppleARMIODevice");
    let mut best = 0u64;
    for service in &services {
        if entry_name(*service) != "pmgr" {
            continue;
        }
        if let Some(bytes) = as_bytes(registry_property(*service, "voltage-states5-sram")) {
            for pair in bytes.chunks_exact(8) {
                let raw = u64::from(u32::from_le_bytes([pair[0], pair[1], pair[2], pair[3]]));
                best = best.max(raw);
            }
        }
    }
    release(services);
    if best > 100_000_000 {
        best / 1_000_000
    } else if best > 100_000 {
        best / 1_000
    } else {
        best
    }
}

fn march() -> &'static str {
    if arm_feature("FEAT_HBC") {
        "ARMv9.3-A"
    } else if arm_feature("FEAT_WFxT") || arm_feature("FEAT_SME") {
        "ARMv9.2-A"
    } else if arm_feature("FEAT_ECV") {
        "ARMv8.6-A"
    } else if arm_feature("FEAT_LSE2") {
        "ARMv8.4-A"
    } else if arm_feature("FEAT_DotProd") {
        "ARMv8.2-A"
    } else {
        "ARMv8-A"
    }
}

fn cpu_module() -> Value {
    let brand = sysctl_string("machdep.cpu.brand_string").unwrap_or_default();
    let physical = sysctl_u64("hw.physicalcpu").unwrap_or(0);
    let logical = sysctl_u64("hw.logicalcpu").unwrap_or(0);
    json!({
        "cpu": brand,
        "vendor": "Apple",
        "cores": {"physical": physical, "logical": logical, "online": logical},
        "frequency": {"base": 0, "max": max_frequency_mhz()},
        "temperature": cpu_temperature(),
        "march": march(),
    })
}

// --- Temperature (IOHIDEventSystemClient, the SMC sensor route) --------------

const HID_PAGE_APPLE_VENDOR: i64 = 0xff00;
const HID_USAGE_TEMPERATURE_SENSOR: i64 = 5;
const HID_EVENT_TYPE_TEMPERATURE: i64 = 15;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDEventSystemClientCreate(allocator: CFTypeRef) -> *mut c_void;
    fn IOHIDEventSystemClientSetMatching(client: *mut c_void, matching: CFTypeRef);
    fn IOHIDEventSystemClientCopyServices(client: *mut c_void) -> CFTypeRef;
    fn IOHIDServiceClientCopyProperty(service: *mut c_void, key: CFTypeRef) -> CFTypeRef;
    fn IOHIDServiceClientCopyEvent(
        service: *mut c_void,
        event_type: i64,
        options: i32,
        timestamp: i64,
    ) -> *mut c_void;
    fn IOHIDEventGetFloatValue(event: *mut c_void, field: i32) -> f64;
}

fn cpu_temperature() -> Option<f64> {
    let mut readings: Vec<f64> = Vec::new();
    unsafe {
        let client = IOHIDEventSystemClientCreate(kCFAllocatorDefault.cast());
        if client.is_null() {
            return None;
        }
        let matching = CFDictionary::from_CFType_pairs(&[
            (
                CFString::new("PrimaryUsagePage").as_CFType(),
                CFNumber::from(HID_PAGE_APPLE_VENDOR).as_CFType(),
            ),
            (
                CFString::new("PrimaryUsage").as_CFType(),
                CFNumber::from(HID_USAGE_TEMPERATURE_SENSOR).as_CFType(),
            ),
        ]);
        IOHIDEventSystemClientSetMatching(client, matching.as_concrete_TypeRef().cast());
        let services = IOHIDEventSystemClientCopyServices(client);
        if services.is_null() {
            return None;
        }
        let services: CFArray<*const c_void> = CFArray::wrap_under_create_rule(services.cast());
        let product_key = CFString::new("Product");
        for service in services.iter() {
            let service = (*service).cast_mut();
            let product =
                IOHIDServiceClientCopyProperty(service, product_key.as_concrete_TypeRef().cast());
            if product.is_null() {
                continue;
            }
            let product = CFType::wrap_under_create_rule(product);
            let Some(name) = product.downcast::<CFString>().map(|s| s.to_string()) else {
                continue;
            };
            // Older Apple Silicon exposes per-cluster sensors (pACC/eACC);
            // newer generations expose die sensors named "PMU tdie<n>".
            if !(name.contains("pACC") || name.contains("eACC") || name.starts_with("PMU tdie")) {
                continue;
            }
            let event = IOHIDServiceClientCopyEvent(service, HID_EVENT_TYPE_TEMPERATURE, 0, 0);
            if event.is_null() {
                continue;
            }
            let value = IOHIDEventGetFloatValue(event, (HID_EVENT_TYPE_TEMPERATURE << 16) as i32);
            core_foundation::base::CFRelease(event.cast());
            if value > 0.0 && value < 150.0 {
                readings.push(value);
            }
        }
    }
    if readings.is_empty() {
        return None;
    }
    Some(readings.iter().sum::<f64>() / readings.len() as f64)
}

// --- GPU ---------------------------------------------------------------------

fn gpu_module() -> Value {
    let mut gpus: Vec<Value> = Vec::new();
    let services = matching_services("IOAccelerator");
    for (index, service) in services.iter().enumerate() {
        let bundle =
            as_string(registry_property(*service, "CFBundleIdentifier")).unwrap_or_default();
        let version = as_string(registry_property(*service, "IOSourceVersion")).unwrap_or_default();
        let mut driver = bundle.clone();
        if !driver.is_empty() && !version.is_empty() {
            driver = format!("{driver} {version}");
        }
        let apple_silicon = bundle.contains("AGX");
        let name = if apple_silicon {
            sysctl_string("machdep.cpu.brand_string").unwrap_or_default()
        } else {
            as_string(search_property(*service, "model")).unwrap_or_default()
        };
        let usage = registry_property(*service, "PerformanceStatistics")
            .and_then(|stats| stats.downcast::<CFDictionary>())
            .and_then(|stats| as_f64(dict_value(&stats, "Device Utilization %")));
        gpus.push(json!({
            "index": index,
            "name": name,
            "vendor": if apple_silicon { "Apple" } else { "" },
            "type": "Integrated",
            "driver": driver,
            "coreCount": as_i64(search_property(*service, "gpu-core-count")),
            "coreUsage": usage,
            "memory": {"dedicated": {"total": Value::Null, "used": Value::Null}},
            "temperature": Value::Null,
        }));
    }
    release(services);
    json!(gpus)
}

// --- Physical disks ----------------------------------------------------------

fn conforms_to(service: io_object_t, class: &str) -> bool {
    let Ok(class) = CString::new(class) else {
        return false;
    };
    unsafe { IOObjectConformsTo(service, class.as_ptr().cast_mut()) != 0 }
}

fn physical_disks() -> Value {
    let mut disks: Vec<Value> = Vec::new();
    let services = matching_services("IOMedia");
    for service in &services {
        if as_bool(registry_property(*service, "Whole")) != Some(true) {
            continue;
        }
        // APFS containers are whole IOMedia objects too, but they are views of
        // a physical disk that is already listed; keeping them would give the
        // machine phantom disks and change its benchmark identity.
        if conforms_to(*service, "AppleAPFSMedia") {
            continue;
        }
        let characteristics = search_property(*service, "Device Characteristics")
            .and_then(|value| value.downcast::<CFDictionary>());
        let medium = characteristics
            .as_ref()
            .and_then(|dict| as_string(dict_value(dict, "Medium Type")))
            .unwrap_or_default();
        let interconnect = search_property(*service, "Protocol Characteristics")
            .and_then(|value| value.downcast::<CFDictionary>())
            .and_then(|dict| as_string(dict_value(&dict, "Physical Interconnect")))
            .unwrap_or_default();
        let device = as_string(registry_property(*service, "BSD Name")).unwrap_or_default();
        disks.push(json!({
            "name": entry_name(*service),
            "devPath": if device.is_empty() { String::new() } else { format!("/dev/{device}") },
            "size": as_i64(registry_property(*service, "Size")).unwrap_or(0),
            "kind": if medium == "Solid State" { "SSD" } else { "HDD" },
            "interconnect": interconnect,
            "removable": as_bool(registry_property(*service, "Removable")).unwrap_or(false),
            "readOnly": as_bool(registry_property(*service, "Writable")) == Some(false),
            "temperature": Value::Null,
        }));
    }
    release(services);
    json!(disks)
}

// --- Battery and power adapter -----------------------------------------------

fn power_modules() -> (Value, Value) {
    let mut batteries: Vec<Value> = Vec::new();
    let mut adapters: Vec<Value> = Vec::new();
    let services = matching_services("AppleSmartBattery");
    for service in &services {
        let external = as_bool(registry_property(*service, "ExternalConnected")) == Some(true);
        let charging = as_bool(registry_property(*service, "IsCharging")) == Some(true);
        let mut status: Vec<&str> = Vec::new();
        if external {
            status.push("AC Connected");
        }
        if charging {
            status.push("Charging");
        }
        batteries.push(json!({
            "modelName": as_string(registry_property(*service, "DeviceName"))
                .unwrap_or_default(),
            "manufacturer": as_string(registry_property(*service, "Manufacturer"))
                .unwrap_or_else(|| "Apple Inc.".to_string()),
            "capacity": as_f64(registry_property(*service, "CurrentCapacity")),
            "status": status,
            "cycleCount": as_i64(registry_property(*service, "CycleCount")),
            "temperature": as_f64(registry_property(*service, "Temperature"))
                .map(|centi| centi / 100.0),
        }));
        if external
            && let Some(details) = registry_property(*service, "AdapterDetails")
                .and_then(|value| value.downcast::<CFDictionary>())
        {
            adapters.push(json!({
                "name": as_string(dict_value(&details, "Name")).unwrap_or_default(),
                "modelName": as_string(dict_value(&details, "Model")).unwrap_or_default(),
                "manufacturer": as_string(dict_value(&details, "Manufacturer"))
                    .unwrap_or_default(),
                "description": as_string(dict_value(&details, "Description"))
                    .unwrap_or_default(),
                "watts": as_i64(dict_value(&details, "Watts")),
            }));
        }
    }
    release(services);
    (json!(batteries), json!(adapters))
}
