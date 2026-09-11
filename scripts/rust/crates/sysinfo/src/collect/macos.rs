use std::ffi::{CString, c_void};
use std::fs;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFAllocatorRef, CFType, CFTypeRef, TCFType, kCFAllocatorDefault};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use io_kit_sys::ret::kIOReturnSuccess;
use io_kit_sys::types::{io_iterator_t, io_object_t};
use io_kit_sys::{
    IOIteratorNext, IOObjectConformsTo, IOObjectRelease, IORegistryEntryCreateCFProperty,
    IORegistryEntryGetName, IORegistryEntrySearchCFProperty, IOServiceGetMatchingServices,
    IOServiceMatching, kIOMasterPortDefault, kIORegistryIterateParents,
    kIORegistryIterateRecursively,
};
use serde_json::{Value, json};
use sysctl::{Ctl, CtlValue, Sysctl};

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
    match Ctl::new(name).ok()?.value().ok()? {
        CtlValue::String(value) => Some(value),
        _ => None,
    }
}

fn sysctl_u64(name: &str) -> Option<u64> {
    unsigned_value(Ctl::new(name).ok()?.value().ok()?)
}

fn unsigned_value(value: CtlValue) -> Option<u64> {
    match value {
        CtlValue::U64(value) | CtlValue::Ulong(value) => Some(value),
        CtlValue::Uint(value) | CtlValue::U32(value) => Some(value.into()),
        CtlValue::U16(value) => Some(value.into()),
        CtlValue::U8(value) => Some(value.into()),
        CtlValue::S64(value) | CtlValue::Long(value) => value.try_into().ok(),
        CtlValue::Int(value) | CtlValue::S32(value) => value.try_into().ok(),
        CtlValue::S16(value) => value.try_into().ok(),
        CtlValue::S8(value) => value.try_into().ok(),
        _ => None,
    }
}

fn arm_feature(name: &str) -> bool {
    sysctl_u64(&format!("hw.optional.arm.{name}")) == Some(1)
}

// --- IOKit helpers -----------------------------------------------------------

struct IoObject(io_object_t);

impl Drop for IoObject {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: only successful owning IOKit calls construct IoObject;
        // it is never copied, and Drop releases that single owned reference.
        unsafe { IOObjectRelease(self.0) };
    }
}

#[allow(unsafe_code)]
fn matching_services(class: &str) -> Vec<IoObject> {
    let Ok(class) = CString::new(class) else {
        return Vec::new();
    };
    // SAFETY: class is NUL-terminated and lives through the call.
    let matching = unsafe { IOServiceMatching(class.as_ptr()) };
    if matching.is_null() {
        return Vec::new();
    }
    let mut iterator: io_iterator_t = 0;
    // SAFETY: matching is nonnull and its owned reference is consumed by this
    // call even on failure; iterator points to initialized writable storage.
    let status =
        unsafe { IOServiceGetMatchingServices(kIOMasterPortDefault, matching, &mut iterator) };
    if status != kIOReturnSuccess {
        return Vec::new();
    }
    let iterator = IoObject(iterator);
    let mut services = Vec::new();
    loop {
        // SAFETY: iterator owns a live iterator; each nonzero result transfers
        // one object reference to the new guard.
        let service = unsafe { IOIteratorNext(iterator.0) };
        if service == 0 {
            break;
        }
        services.push(IoObject(service));
    }
    services
}

#[allow(unsafe_code)]
fn registry_property(entry: &IoObject, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    // SAFETY: entry is owned and key is a live CFString. A nonnull result is
    // a CF object with a +1 reference, transferred to the owning CFType below.
    let value = unsafe {
        IORegistryEntryCreateCFProperty(entry.0, key.as_concrete_TypeRef(), kCFAllocatorDefault, 0)
    };
    if value.is_null() {
        return None;
    }
    // SAFETY: the checked nonnull result follows the CF create ownership rule.
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

#[allow(unsafe_code)]
fn search_property(entry: &IoObject, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    let plane = c"IOService";
    // SAFETY: entry is owned; key and plane remain live for the call. Search
    // returns an owned CF object when nonnull, like CreateCFProperty.
    let value = unsafe {
        IORegistryEntrySearchCFProperty(
            entry.0,
            plane.as_ptr(),
            key.as_concrete_TypeRef(),
            kCFAllocatorDefault,
            kIORegistryIterateRecursively | kIORegistryIterateParents,
        )
    };
    if value.is_null() {
        return None;
    }
    // SAFETY: the checked nonnull result carries the caller's +1 reference.
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

#[allow(unsafe_code)]
fn dict_value(dict: &CFDictionary, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    let raw = dict.find(key.as_concrete_TypeRef() as *const c_void)?;
    // SAFETY: these dictionaries come from IOKit CF properties, whose values
    // are CF objects. dict keeps the value alive; the get rule retains it.
    Some(unsafe { CFType::wrap_under_get_rule(*raw as CFTypeRef) })
}

#[allow(unsafe_code)]
fn entry_name(entry: &IoObject) -> String {
    let mut buffer = [0u8; 128];
    // SAFETY: entry is owned; io_name_t is a 128-byte output array. Conversion
    // below is bounded even if the returned bytes have no terminating NUL.
    if unsafe { IORegistryEntryGetName(entry.0, buffer.as_mut_ptr().cast()) } != kIOReturnSuccess {
        return String::new();
    }
    std::ffi::CStr::from_bytes_until_nul(&buffer)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
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
    let names = rustix::system::uname();
    json!({
        "name": names.sysname().to_string_lossy(),
        "release": names.release().to_string_lossy(),
        "version": names.version().to_string_lossy(),
        "architecture": names.machine().to_string_lossy(),
    })
}

// --- CPU ---------------------------------------------------------------------

fn max_frequency_mhz() -> u64 {
    let services = matching_services("AppleARMIODevice");
    let mut best = 0u64;
    for service in &services {
        if entry_name(service) != "pmgr" {
            continue;
        }
        if let Some(bytes) = as_bytes(registry_property(service, "voltage-states5-sram")) {
            for pair in bytes.chunks_exact(8) {
                let raw = u64::from(u32::from_le_bytes([pair[0], pair[1], pair[2], pair[3]]));
                best = best.max(raw);
            }
        }
    }
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
#[allow(unsafe_code)]
unsafe extern "C" {
    fn IOHIDEventSystemClientCreate(allocator: CFAllocatorRef) -> *mut c_void;
    fn IOHIDEventSystemClientSetMatching(client: *mut c_void, matching: CFDictionaryRef);
    fn IOHIDEventSystemClientCopyServices(client: *mut c_void) -> CFArrayRef;
    fn IOHIDServiceClientCopyProperty(service: *mut c_void, key: CFStringRef) -> CFTypeRef;
    fn IOHIDServiceClientCopyEvent(
        service: *mut c_void,
        event_type: i64,
        options: i32,
        timestamp: i64,
    ) -> *mut c_void;
    fn IOHIDEventGetFloatValue(event: *mut c_void, field: i32) -> f64;
}

struct HidClient(CFType);

impl HidClient {
    #[allow(unsafe_code)]
    fn new() -> Option<Self> {
        // SAFETY: the default allocator is valid; Create returns a +1 CF
        // reference, transferred into the guard after the null check.
        let client = unsafe { IOHIDEventSystemClientCreate(kCFAllocatorDefault) };
        if client.is_null() {
            return None;
        }
        // SAFETY: client is the checked, owned CF object returned by Create.
        Some(Self(unsafe {
            CFType::wrap_under_create_rule(client.cast())
        }))
    }

    #[allow(unsafe_code)]
    fn temperature_services(&self) -> Option<Vec<HidService>> {
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
        let client = self.0.as_CFTypeRef().cast_mut();
        // SAFETY: self owns this HID client and matching is a live dictionary.
        unsafe { IOHIDEventSystemClientSetMatching(client, matching.as_concrete_TypeRef()) };
        // SAFETY: client remains alive. CopyServices returns an owned CFArray
        // of HID service clients; it does not transfer ownership of self.
        let services = unsafe { IOHIDEventSystemClientCopyServices(client) };
        if services.is_null() {
            return None;
        }
        // SAFETY: CopyServices returns a +1 CFArray whose elements are HID
        // CF objects, so CFType is the valid element type for this array.
        let services: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(services) };
        Some(
            services
                .iter()
                .map(|service| HidService((*service).clone()))
                .collect(),
        )
    }
}

struct HidService(CFType);

impl HidService {
    #[allow(unsafe_code)]
    fn product(&self) -> Option<String> {
        let key = CFString::new("Product");
        // SAFETY: self owns a HID service; key is a live CFString. CopyProperty
        // returns either null or one owned reference to a CF object.
        let product = unsafe {
            IOHIDServiceClientCopyProperty(
                self.0.as_CFTypeRef().cast_mut(),
                key.as_concrete_TypeRef(),
            )
        };
        if product.is_null() {
            return None;
        }
        // SAFETY: product is the checked +1 result of CopyProperty.
        let product = unsafe { CFType::wrap_under_create_rule(product) };
        product.downcast::<CFString>().map(|name| name.to_string())
    }

    #[allow(unsafe_code)]
    fn temperature(&self) -> Option<f64> {
        // SAFETY: self owns a HID service. The retained event uses the same
        // private Apple temperature ABI as the existing collector (type15).
        let event = unsafe {
            IOHIDServiceClientCopyEvent(
                self.0.as_CFTypeRef().cast_mut(),
                HID_EVENT_TYPE_TEMPERATURE,
                0,
                0,
            )
        };
        if event.is_null() {
            return None;
        }
        // SAFETY: CopyEvent returns a +1 CF object, now owned by this guard.
        let event = unsafe { CFType::wrap_under_create_rule(event.cast()) };
        // SAFETY: event is live and the field belongs to its temperature type.
        Some(unsafe {
            IOHIDEventGetFloatValue(
                event.as_CFTypeRef().cast_mut(),
                (HID_EVENT_TYPE_TEMPERATURE << 16) as i32,
            )
        })
    }
}

fn cpu_temperature() -> Option<f64> {
    let client = HidClient::new()?;
    let services = client.temperature_services()?;
    average_temperature(services.iter().filter_map(|service| {
        let name = service.product()?;
        if !cpu_sensor(&name) {
            return None;
        }
        service.temperature()
    }))
}

fn cpu_sensor(name: &str) -> bool {
    name.contains("pACC") || name.contains("eACC") || name.starts_with("PMU tdie")
}

fn average_temperature(readings: impl IntoIterator<Item = f64>) -> Option<f64> {
    let (total, count) = readings
        .into_iter()
        .filter(|&value| value > 0.0 && value < 150.0)
        .fold((0.0, 0usize), |(total, count), value| {
            (total + value, count + 1)
        });
    (count > 0).then(|| total / count as f64)
}

// --- GPU ---------------------------------------------------------------------

fn gpu_module() -> Value {
    let mut gpus: Vec<Value> = Vec::new();
    let services = matching_services("IOAccelerator");
    for (index, service) in services.iter().enumerate() {
        let bundle =
            as_string(registry_property(service, "CFBundleIdentifier")).unwrap_or_default();
        let version = as_string(registry_property(service, "IOSourceVersion")).unwrap_or_default();
        let mut driver = bundle.clone();
        if !driver.is_empty() && !version.is_empty() {
            driver = format!("{driver} {version}");
        }
        let apple_silicon = bundle.contains("AGX");
        let name = if apple_silicon {
            sysctl_string("machdep.cpu.brand_string").unwrap_or_default()
        } else {
            as_string(search_property(service, "model")).unwrap_or_default()
        };
        let usage = registry_property(service, "PerformanceStatistics")
            .and_then(|stats| stats.downcast::<CFDictionary>())
            .and_then(|stats| as_f64(dict_value(&stats, "Device Utilization %")));
        gpus.push(json!({
            "index": index,
            "name": name,
            "vendor": if apple_silicon { "Apple" } else { "" },
            "type": "Integrated",
            "driver": driver,
            "coreCount": as_i64(search_property(service, "gpu-core-count")),
            "coreUsage": usage,
            "memory": {"dedicated": {"total": Value::Null, "used": Value::Null}},
            "temperature": Value::Null,
        }));
    }
    json!(gpus)
}

// --- Physical disks ----------------------------------------------------------

#[allow(unsafe_code)]
fn conforms_to(service: &IoObject, class: &str) -> bool {
    let Ok(class) = CString::new(class) else {
        return false;
    };
    // SAFETY: service is owned and class is NUL-terminated; IOKit treats the
    // historical mutable class pointer as an input string.
    unsafe { IOObjectConformsTo(service.0, class.as_ptr().cast_mut()) != 0 }
}

fn physical_disks() -> Value {
    let mut disks: Vec<Value> = Vec::new();
    let services = matching_services("IOMedia");
    for service in &services {
        if as_bool(registry_property(service, "Whole")) != Some(true) {
            continue;
        }
        // APFS containers are whole IOMedia objects too, but they are views of
        // a physical disk that is already listed; keeping them would give the
        // machine phantom disks and change its benchmark identity.
        if conforms_to(service, "AppleAPFSMedia") {
            continue;
        }
        let characteristics = search_property(service, "Device Characteristics")
            .and_then(|value| value.downcast::<CFDictionary>());
        let medium = characteristics
            .as_ref()
            .and_then(|dict| as_string(dict_value(dict, "Medium Type")))
            .unwrap_or_default();
        let interconnect = search_property(service, "Protocol Characteristics")
            .and_then(|value| value.downcast::<CFDictionary>())
            .and_then(|dict| as_string(dict_value(&dict, "Physical Interconnect")))
            .unwrap_or_default();
        let device = as_string(registry_property(service, "BSD Name")).unwrap_or_default();
        disks.push(json!({
            "name": entry_name(service),
            "devPath": if device.is_empty() { String::new() } else { format!("/dev/{device}") },
            "size": as_i64(registry_property(service, "Size")).unwrap_or(0),
            "kind": if medium == "Solid State" { "SSD" } else { "HDD" },
            "interconnect": interconnect,
            "removable": as_bool(registry_property(service, "Removable")).unwrap_or(false),
            "readOnly": as_bool(registry_property(service, "Writable")) == Some(false),
            "temperature": Value::Null,
        }));
    }
    json!(disks)
}

// --- Battery and power adapter -----------------------------------------------

fn power_modules() -> (Value, Value) {
    let mut batteries: Vec<Value> = Vec::new();
    let mut adapters: Vec<Value> = Vec::new();
    let services = matching_services("AppleSmartBattery");
    for service in &services {
        let external = as_bool(registry_property(service, "ExternalConnected")) == Some(true);
        let charging = as_bool(registry_property(service, "IsCharging")) == Some(true);
        let mut status: Vec<&str> = Vec::new();
        if external {
            status.push("AC Connected");
        }
        if charging {
            status.push("Charging");
        }
        batteries.push(json!({
            "modelName": as_string(registry_property(service, "DeviceName"))
                .unwrap_or_default(),
            "manufacturer": as_string(registry_property(service, "Manufacturer"))
                .unwrap_or_else(|| "Apple Inc.".to_string()),
            "capacity": as_f64(registry_property(service, "CurrentCapacity")),
            "status": status,
            "cycleCount": as_i64(registry_property(service, "CycleCount")),
            "temperature": as_f64(registry_property(service, "Temperature"))
                .map(|centi| centi / 100.0),
        }));
        if external
            && let Some(details) = registry_property(service, "AdapterDetails")
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
    (json!(batteries), json!(adapters))
}

#[cfg(test)]
#[path = "../../tests/macos_tests.rs"]
mod tests;
