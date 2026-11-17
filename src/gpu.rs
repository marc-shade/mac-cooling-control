//! Live Apple GPU utilization from the IOKit accelerator statistics.
//! Missing counters are errors, never idle samples.
use anyhow::{Result, bail};

/// Public NSProcessInfo thermal state, kept as a state rather than Celsius.
#[cfg(target_os = "macos")]
pub fn thermal_pressure() -> Result<u8> {
    use std::ffi::{c_char, c_void};
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    // Objective-C dispatch requires signatures matching each selector's ABI.
    unsafe {
        let class = objc_getClass(c"NSProcessInfo".as_ptr());
        if class.is_null() {
            bail!("NSProcessInfo unavailable");
        }
        let object_send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let state_send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let info = object_send(class, sel_registerName(c"processInfo".as_ptr()));
        if info.is_null() {
            bail!("NSProcessInfo instance unavailable");
        }
        let state = state_send(info, sel_registerName(c"thermalState".as_ptr()));
        if !(0..=3).contains(&state) {
            bail!("unknown thermal pressure state: {state}");
        }
        Ok(state as u8)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn thermal_pressure() -> Result<u8> {
    bail!("macOS thermal pressure unavailable")
}

pub fn pressure_name(state: u8) -> &'static str {
    match state {
        0 => "nominal",
        1 => "fair",
        2 => "serious",
        3 => "critical",
        _ => "unknown",
    }
}

pub fn validate_percent(value: f64) -> Result<f64> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        bail!("GPU utilization outside 0..100 percent: {value}");
    }
    Ok(value)
}

#[cfg(target_os = "macos")]
pub fn read_percent() -> Result<f64> {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    use std::ffi::{c_char, c_void};
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingServices(port: u32, matching: *mut c_void, iter: *mut u32) -> i32;
        fn IOIteratorNext(iter: u32) -> u32;
        fn IOObjectRelease(object: u32) -> i32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: CFTypeRef,
            allocator: CFTypeRef,
            options: u32,
        ) -> CFTypeRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFGetTypeID(value: CFTypeRef) -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFNumberGetValue(number: CFTypeRef, kind: i32, value: *mut f64) -> bool;
    }
    let stats_key = CFString::from_static_string("PerformanceStatistics");
    let utilization_key = CFString::from_static_string("Device Utilization %");
    let mut iter = 0;
    let mut maximum: Option<f64> = None;
    // IOServiceGetMatchingServices consumes the matching dictionary. Every
    // returned IO object/property is released below before validation returns.
    unsafe {
        let matching = IOServiceMatching(c"AGXAccelerator".as_ptr());
        if matching.is_null() {
            bail!("cannot create GPU registry match");
        }
        let result = IOServiceGetMatchingServices(0, matching, &mut iter);
        if result != 0 {
            bail!("GPU registry enumeration failed: {result:#x}");
        }
        loop {
            let service = IOIteratorNext(iter);
            if service == 0 {
                break;
            }
            let stats = IORegistryEntryCreateCFProperty(
                service,
                stats_key.as_CFTypeRef(),
                std::ptr::null(),
                0,
            );
            IOObjectRelease(service);
            if stats.is_null() {
                continue;
            }
            if CFGetTypeID(stats) == CFDictionaryGetTypeID() {
                let number = CFDictionaryGetValue(stats, utilization_key.as_CFTypeRef());
                let mut value = 0.0;
                // kCFNumberFloat64Type = 6, from CFNumber.h.
                if !number.is_null()
                    && CFGetTypeID(number) == CFNumberGetTypeID()
                    && CFNumberGetValue(number, 6, &mut value)
                {
                    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                        CFRelease(stats);
                        IOObjectRelease(iter);
                        return validate_percent(value);
                    }
                    maximum = Some(maximum.map_or(value, |previous| previous.max(value)));
                }
            }
            CFRelease(stats);
        }
        IOObjectRelease(iter);
    }
    maximum.ok_or_else(|| anyhow::anyhow!("GPU Device Utilization counter is unavailable"))
}

#[cfg(not(target_os = "macos"))]
pub fn read_percent() -> Result<f64> {
    bail!("Apple GPU telemetry requires macOS")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_or_invalid_gpu_is_not_idle() {
        for value in [f64::NAN, f64::INFINITY, -1.0, 101.0] {
            assert!(validate_percent(value).is_err());
        }
        assert_eq!(validate_percent(0.0).unwrap(), 0.0);
        assert_eq!(validate_percent(100.0).unwrap(), 100.0);
    }
}
