//! macOS die temperature reading.
//! SMC die sensors through IOHIDEventSystemClient. Unavailable sensors are
//! errors; thermal pressure is not a temperature measurement.

use anyhow::{Result, bail};
use core_foundation::array::CFArray;
use core_foundation::base::{CFRelease, CFTypeRef, TCFType, kCFAllocatorDefault};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;

use super::Reading;

// kHIDPage_AppleVendor / kHIDUsage_AppleVendor_TemperatureSensor
const APPLE_VENDOR_PAGE: i32 = 0xff00;
const TEMPERATURE_SENSOR_USAGE: i32 = 5;
// IOHIDEventTypeTemperature; field base = type << 16
const EVENT_TYPE_TEMPERATURE: i64 = 15;
const TEMPERATURE_FIELD: u32 = (EVENT_TYPE_TEMPERATURE as u32) << 16;

#[repr(C)]
struct Opaque {
    _private: [u8; 0],
}
type IOHIDEventSystemClientRef = *mut Opaque;
type IOHIDServiceClientRef = *mut Opaque;
type IOHIDEventRef = *mut Opaque;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDEventSystemClientCreate(allocator: *const c_void) -> IOHIDEventSystemClientRef;
    fn IOHIDEventSystemClientSetMatching(
        client: IOHIDEventSystemClientRef,
        matching: CFDictionaryRef,
    ) -> i32;
    fn IOHIDEventSystemClientCopyServices(client: IOHIDEventSystemClientRef) -> *const c_void;
    fn IOHIDServiceClientCopyProperty(
        service: IOHIDServiceClientRef,
        key: CFStringRef,
    ) -> CFTypeRef;
    fn IOHIDServiceClientCopyEvent(
        service: IOHIDServiceClientRef,
        event_type: i64,
        options: i32,
        timestamp: i64,
    ) -> IOHIDEventRef;
    fn IOHIDEventGetFloatValue(event: IOHIDEventRef, field: u32) -> f64;
}

pub struct Reader {
    client: IOHIDEventSystemClientRef,
}

/// Apple Silicon CPU-core die sensors are named like
/// "pACC MTR Temp Sensor4" (P-cores) / "eACC MTR Temp Sensor1" (E-cores).
fn is_die_sensor(name: &str) -> bool {
    name.contains("MTR Temp Sensor") || name.starts_with("PMU tdie")
}

fn hottest_die(readings: &[Reading]) -> Result<f64> {
    readings
        .iter()
        .filter(|r| is_die_sensor(&r.name))
        .map(|r| r.celsius)
        .filter(|v| v.is_finite() && (0.0..=125.0).contains(v))
        .reduce(f64::max)
        .ok_or_else(|| {
            anyhow::anyhow!("no readable die temperature sensors; refusing estimated temperatures")
        })
}

impl Reader {
    pub fn new() -> Result<Self> {
        let client = unsafe { IOHIDEventSystemClientCreate(kCFAllocatorDefault) };
        if !client.is_null() {
            let matching = CFDictionary::from_CFType_pairs(&[
                (
                    CFString::from_static_string("PrimaryUsagePage").as_CFType(),
                    CFNumber::from(APPLE_VENDOR_PAGE).as_CFType(),
                ),
                (
                    CFString::from_static_string("PrimaryUsage").as_CFType(),
                    CFNumber::from(TEMPERATURE_SENSOR_USAGE).as_CFType(),
                ),
            ]);
            unsafe {
                IOHIDEventSystemClientSetMatching(client, matching.as_concrete_TypeRef());
            }
            let reader = Reader { client };
            reader.read()?;
            return Ok(reader);
        }
        bail!("cannot create temperature sensor client")
    }

    pub fn read(&self) -> Result<f64> {
        hottest_die(&self.smc_readings()?)
    }

    pub fn source_name(&self) -> &'static str {
        "SMC hottest die sensor"
    }

    pub fn is_fallback(&self) -> bool {
        false
    }

    pub fn sensors(&self) -> Result<Vec<Reading>> {
        let mut readings = self.smc_readings()?;
        readings.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(readings)
    }

    fn smc_readings(&self) -> Result<Vec<Reading>> {
        if self.client.is_null() {
            bail!("no IOHIDEventSystemClient");
        }
        let services = unsafe { IOHIDEventSystemClientCopyServices(self.client) };
        if services.is_null() {
            bail!("no SMC sensor services found");
        }
        let services: CFArray<*const c_void> =
            unsafe { CFArray::wrap_under_create_rule(services as _) };

        let product_key = CFString::from_static_string("Product");
        let mut readings = Vec::new();

        for service in services.iter() {
            let service = *service as IOHIDServiceClientRef;
            let name = unsafe {
                let prop =
                    IOHIDServiceClientCopyProperty(service, product_key.as_concrete_TypeRef());
                if prop.is_null() {
                    continue;
                }
                let s = CFString::wrap_under_create_rule(prop as _);
                s.to_string()
            };
            let event =
                unsafe { IOHIDServiceClientCopyEvent(service, EVENT_TYPE_TEMPERATURE, 0, 0) };
            if event.is_null() {
                continue;
            }
            let value = unsafe {
                let v = IOHIDEventGetFloatValue(event, TEMPERATURE_FIELD);
                CFRelease(event as CFTypeRef);
                v
            };
            if !(0.0..=125.0).contains(&value) {
                continue;
            }
            readings.push(Reading {
                name,
                celsius: value,
            });
        }

        if readings.is_empty() {
            bail!("no usable temperature sensors");
        }
        Ok(readings)
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if !self.client.is_null() {
            unsafe { CFRelease(self.client as CFTypeRef) };
        }
    }
}

// SAFETY: the client is only used from the curve loop's single thread.
unsafe impl Send for Reader {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_hottest_die_and_excludes_unrelated_sensors() {
        let rows = [
            ("PMU tdie1", 61.0),
            ("PMU tdie2", 73.0),
            ("gas gauge battery", 30.0),
            ("NAND CH0 temp", 95.0),
            ("PMU tcal", 110.0),
            ("PMU tdie3", f64::NAN),
        ]
        .map(|(name, celsius)| Reading {
            name: name.into(),
            celsius,
        });
        assert_eq!(hottest_die(&rows).unwrap(), 73.0);
        assert!(hottest_die(&rows[2..]).is_err());
        assert!(hottest_die(&[]).is_err());
        assert!(is_die_sensor("pACC MTR Temp Sensor4"));
    }
}
