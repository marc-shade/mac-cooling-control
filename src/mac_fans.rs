//! Read the Mac's actual fan tachometers. Zero is a valid reading.
use anyhow::Result;

#[cfg(not(target_os = "macos"))]
pub fn read_rpm() -> Result<Vec<u32>> {
    anyhow::bail!("Mac fan tachometers require macOS")
}

#[cfg(target_os = "macos")]
pub use mac::read_rpm;

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use anyhow::{Context, ensure};
    use std::ffi::{c_char, c_void};
    use std::ptr;

    // AppleSMC user client selector 2 exchanges this fixed-size record.
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct KeyInfo {
        size: u32,
        kind: u32,
        attributes: u8,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Request {
        key: u32,
        version_and_power_limits: [u8; 24],
        info: KeyInfo,
        result: u8,
        status: u8,
        operation: u8,
        index: u32,
        bytes: [u8; 32],
    }

    const _: () = assert!(size_of::<Request>() == 80 && std::mem::offset_of!(Request, bytes) == 48);

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(port: u32, matching: *mut c_void) -> u32;
        fn IOObjectRelease(object: u32) -> i32;
        fn IOServiceOpen(service: u32, task: u32, kind: u32, connection: *mut u32) -> i32;
        fn IOServiceClose(connection: u32) -> i32;
        fn IOConnectCallScalarMethod(
            connection: u32,
            selector: u32,
            input: *const u64,
            count: u32,
            output: *mut u64,
            out_count: *mut u32,
        ) -> i32;
        fn IOConnectCallStructMethod(
            connection: u32,
            selector: u32,
            input: *const Request,
            input_size: usize,
            output: *mut Request,
            output_size: *mut usize,
        ) -> i32;
    }

    struct Smc(u32);

    impl Smc {
        fn open() -> Result<Self> {
            // IOKit consumes the matching dictionary. Balance every service and connection.
            unsafe {
                let matching = IOServiceMatching(c"AppleSMC".as_ptr());
                ensure!(!matching.is_null(), "AppleSMC match unavailable");
                let service = IOServiceGetMatchingService(0, matching);
                ensure!(service != 0, "AppleSMC service unavailable");
                let mut port = 0;
                let status = IOServiceOpen(service, mach_task_self_, 1, &mut port);
                IOObjectRelease(service);
                ensure!(status == 0, "opening AppleSMC: {status:#x}");
                let status = IOConnectCallScalarMethod(
                    port,
                    0,
                    ptr::null(),
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                if status != 0 {
                    IOServiceClose(port);
                    anyhow::bail!("opening SMC user client: {status:#x}");
                }
                Ok(Self(port))
            }
        }

        fn call(&self, input: &Request) -> Result<Request> {
            let mut output = Request::default();
            let mut size = size_of::<Request>();
            let status = unsafe {
                IOConnectCallStructMethod(self.0, 2, input, size, &mut output, &mut size)
            };
            ensure!(
                status == 0 && size == size_of::<Request>() && output.result == 0,
                "SMC response status {status:#x}, result {}, size {size}",
                output.result
            );
            Ok(output)
        }

        fn number(&self, key: [u8; 4]) -> Result<f64> {
            let mut request = Request {
                key: u32::from_be_bytes(key),
                operation: 9,
                ..Default::default()
            };
            request.info = self.call(&request)?.info;
            request.operation = 5;
            let value = self.call(&request)?.bytes;
            match (&request.info.kind.to_be_bytes(), request.info.size) {
                (b"ui8 ", 1) => Ok(f64::from(value[0])),
                (b"flt ", 4) => Ok(f64::from(f32::from_ne_bytes([
                    value[0], value[1], value[2], value[3],
                ]))),
                (b"fpe2", 2) => Ok(f64::from(u16::from_be_bytes([value[0], value[1]])) / 4.0),
                _ => anyhow::bail!(
                    "unsupported SMC type or size for {}",
                    String::from_utf8_lossy(&key)
                ),
            }
        }
    }

    impl Drop for Smc {
        fn drop(&mut self) {
            unsafe {
                IOConnectCallScalarMethod(
                    self.0,
                    1,
                    ptr::null(),
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                IOServiceClose(self.0);
            }
        }
    }

    pub fn read_rpm() -> Result<Vec<u32>> {
        let smc = Smc::open()?;
        let count = smc.number(*b"FNum").context("reading fan count")?;
        ensure!(
            (1.0..=10.0).contains(&count) && count.fract() == 0.0,
            "invalid or absent fan count: {count}"
        );
        (0..count as u8)
            .map(|i| {
                let rpm = smc
                    .number([b'F', b'0' + i, b'A', b'c'])
                    .with_context(|| format!("reading F{i}Ac"))?;
                ensure!(
                    (0.0..=20000.0).contains(&rpm),
                    "invalid F{i}Ac tachometer: {rpm}"
                );
                Ok(rpm.round() as u32)
            })
            .collect()
    }
}
