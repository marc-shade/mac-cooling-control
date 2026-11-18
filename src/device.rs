//! Discovery and I/O for the cooling pad's HID control interface.
//!
//! The pad exposes two HID interfaces, both usage page 0x0C / usage 1 on
//! macOS. Interface 0 is the Razer control interface (feature reports only,
//! MaxFeatureReportSize=90); interface 1 carries media-key input events and
//! must never be opened (it belongs to Apple's HID event driver, and opening
//! it can trip the Input Monitoring permission gate).
//!
//! [`Pad`] owns the command sequencing (inter-command pacing, busy retries,
//! `--verify` echo checks) and talks to the device through the [`Transport`]
//! trait, so that logic is unit-testable against a scripted fake.

use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use hidapi::{DeviceInfo, HidApi, HidDevice};

use crate::packet::{Packet, REPORT_LEN, Response, Status};

pub const VID: u16 = 0x1532;
pub const PID: u16 = 0x0F43;

/// Pause between consecutive commands; Razer firmware can wedge on
/// rapid back-to-back feature reports.
#[cfg(not(test))]
const INTER_COMMAND_DELAY: Duration = Duration::from_millis(50);
#[cfg(test)]
const INTER_COMMAND_DELAY: Duration = Duration::from_millis(1);
/// Wait between sending a 0x8x query and reading its response.
#[cfg(not(test))]
const QUERY_DELAY: Duration = Duration::from_millis(100);
#[cfg(test)]
const QUERY_DELAY: Duration = Duration::from_millis(1);
const BUSY_RETRIES: u32 = 3;

pub fn api() -> Result<HidApi> {
    HidApi::new().context("initializing hidapi")
}

/// Raw feature-report exchange with the device. Implemented by [`HidDevice`]
/// for real hardware and by scripted fakes in tests.
pub trait Transport {
    /// Send a full 91-byte feature report (report id + packet).
    fn send_report(&self, report: &[u8; REPORT_LEN]) -> Result<()>;
    /// Read the device's current 91-byte feature report.
    fn read_report(&self) -> Result<[u8; REPORT_LEN]>;
}

impl Transport for HidDevice {
    fn send_report(&self, report: &[u8; REPORT_LEN]) -> Result<()> {
        self.send_feature_report(report)
            .context("sending feature report")?;
        Ok(())
    }

    fn read_report(&self) -> Result<[u8; REPORT_LEN]> {
        let mut buf = [0u8; REPORT_LEN];
        let n = self
            .get_feature_report(&mut buf)
            .context("reading feature report")?;
        if n < REPORT_LEN - 1 {
            bail!("short feature report: {n} bytes, expected {REPORT_LEN}");
        }
        Ok(buf)
    }
}

/// Narrow device discovery to a specific pad when several are connected.
#[derive(Debug, Default, Clone)]
pub struct Selector {
    /// Match the USB serial number reported by the device descriptor.
    pub serial: Option<String>,
    /// Match the exact HID path shown by `padctl list`.
    pub path: Option<String>,
}

impl Selector {
    fn matches(&self, d: &DeviceInfo) -> bool {
        if let Some(path) = &self.path
            && d.path().to_string_lossy() != *path
        {
            return false;
        }
        if let Some(serial) = &self.serial
            && d.serial_number() != Some(serial.as_str())
        {
            return false;
        }
        true
    }

    fn describe(&self) -> String {
        match (&self.serial, &self.path) {
            (None, None) => String::new(),
            (Some(s), None) => format!(" matching serial {s}"),
            (None, Some(p)) => format!(" matching path {p}"),
            (Some(s), Some(p)) => format!(" matching serial {s} and path {p}"),
        }
    }
}

/// How to open the pad; carried by every command from the global CLI flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenOpts {
    /// After each command, read back the device status and fail loudly if
    /// the device rejected it or the acknowledgement does not match.
    pub verify: bool,
}

fn candidates<'a>(api: &'a HidApi, selector: &Selector) -> Vec<&'a DeviceInfo> {
    let mut seen = std::collections::HashSet::new();
    api.device_list()
        .filter(|d| d.vendor_id() == VID && d.product_id() == PID)
        .filter(|d| selector.matches(d))
        .filter(|d| seen.insert(d.path().to_owned()))
        .collect()
}

pub struct Pad {
    dev: Box<dyn Transport>,
    verify: bool,
    // Held for the lifetime of the connection, including between curve polls.
    _lock: Option<std::fs::File>,
}

impl Pad {
    /// Open the control interface: prefer bInterfaceNumber 0, and verify by
    /// probing a feature-report read (interface 1 has MaxFeatureReportSize=1
    /// and fails the 91-byte read).
    pub fn open(api: &HidApi, selector: &Selector, opts: OpenOpts) -> Result<Pad> {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        let cache = if cfg!(target_os = "macos") {
            std::path::PathBuf::from(home).join("Library/Caches/padctl")
        } else {
            std::path::PathBuf::from(home).join(".cache/padctl")
        };
        std::fs::create_dir_all(&cache).context("creating padctl lock directory")?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(cache.join("device.lock"))
            .context("opening padctl device lock")?;
        lock.try_lock().context("another padctl process owns the controller; stop its curve/service before issuing device commands")?;
        let mut cands = candidates(api, selector);
        if cands.is_empty() {
            bail!(
                "no Razer Laptop Cooling Pad (1532:0f43) found{} — is it plugged in? \
                 Check with `padctl list` or `system_profiler SPUSBDataType`",
                selector.describe()
            );
        }
        cands.sort_by_key(|d| match d.interface_number() {
            0 => 0,
            -1 => 1,
            n => 1 + n,
        });

        let mut errors = Vec::new();
        for info in cands {
            let path = info.path().to_string_lossy().into_owned();
            match api.open_path(info.path()) {
                Ok(dev) => match Transport::read_report(&dev) {
                    Ok(_) => {
                        log::debug!(
                            "opened control interface {} ({})",
                            info.interface_number(),
                            path
                        );
                        let mut pad = Pad::with_transport(Box::new(dev), opts);
                        pad._lock = Some(lock);
                        return Ok(pad);
                    }
                    Err(e) => errors.push(format!("{path}: {e:#}")),
                },
                Err(e) => errors.push(format!("{path}: open failed: {e}")),
            }
        }
        Err(anyhow!(
            "found the cooling pad but could not open its control interface:\n  {}\n\
             If the error mentions permissions, grant your terminal Input Monitoring in \
             System Settings → Privacy & Security. If the device seems stuck, unplug and \
             replug it, or quit Razer Synapse.",
            errors.join("\n  ")
        ))
    }

    /// Wrap an already-open transport (tests use this with a scripted fake).
    pub fn with_transport(dev: Box<dyn Transport>, opts: OpenOpts) -> Pad {
        Pad {
            dev,
            verify: opts.verify,
            _lock: None,
        }
    }

    /// Send a command packet. With `--verify`, read back the device status
    /// afterwards and fail if the command was rejected.
    pub fn send(&self, packet: &Packet) -> Result<()> {
        let report = packet.to_report();
        self.send_report(&report)?;
        if self.verify {
            self.confirm(&report)?;
        }
        Ok(())
    }

    /// Send a pre-built 91-byte feature report verbatim (report id + packet).
    pub fn send_report(&self, report: &[u8; REPORT_LEN]) -> Result<()> {
        log::debug!("-> {}", hex(&report[1..]));
        self.dev.send_report(report)?;
        sleep(INTER_COMMAND_DELAY);
        Ok(())
    }

    /// Send a pre-built report while preserving the global `--verify` behavior.
    pub fn send_raw_report(&self, report: &[u8; REPORT_LEN]) -> Result<()> {
        self.send_report(report)?;
        if self.verify {
            self.confirm(report)?;
        }
        Ok(())
    }

    /// Read the device's current 91-byte feature report without sending
    /// a request first (the Windows plugin reads RPM this way).
    pub fn read_report(&self) -> Result<[u8; REPORT_LEN]> {
        let buf = self.dev.read_report()?;
        log::debug!("<- {}", hex(&buf[1..]));
        Ok(buf)
    }

    /// Require an acknowledgement matching the command just sent.
    fn confirm(&self, sent: &[u8; REPORT_LEN]) -> Result<()> {
        for _ in 0..=BUSY_RETRIES {
            sleep(QUERY_DELAY);
            let report = self.read_report()?;
            let resp = Response::from_report(&report);
            if report[89] != crate::packet::crc(&report[1..]) {
                bail!("verify: invalid response checksum");
            }
            if resp.transaction_id != sent[2] {
                bail!("verify: response transaction does not match");
            }
            if resp.class != sent[7] || resp.cmd != sent[8] {
                bail!(
                    "verify: device did not echo the command \
                     (got class 0x{:02x} cmd 0x{:02x})",
                    resp.class,
                    resp.cmd
                );
            }
            match resp.status {
                Status::Ok => {
                    if resp.class == 0x0d && report[9..13] != sent[9..13] {
                        bail!("verify: fan acknowledgement contains a different command value");
                    }
                    if resp.class == 0x0f
                        && resp.cmd == 0x02
                        && matches!(sent[11], 0 | 1)
                        && report[9..18] != sent[9..18]
                    {
                        bail!(
                            "verify: lighting acknowledgement contains a different effect or color"
                        );
                    }
                    return Ok(());
                }
                Status::Busy => continue,
                status => bail!("device rejected the command: {status}"),
            }
        }
        bail!("device still busy while verifying the command")
    }

    /// Send a query (0x8x command) and read back its response, retrying
    /// while the device reports busy.
    pub fn query(&self, packet: &Packet) -> Result<Response> {
        for attempt in 0..=BUSY_RETRIES {
            self.send_report(&packet.to_report())?;
            sleep(QUERY_DELAY);
            let report = self.read_report()?;
            let resp = Response::from_report(&report);
            match resp.status {
                Status::Busy if attempt < BUSY_RETRIES => {
                    sleep(QUERY_DELAY);
                    continue;
                }
                Status::Ok | Status::New => {
                    let sent = packet.to_report();
                    if self.verify
                        && (resp.status != Status::Ok
                            || resp.transaction_id != sent[2]
                            || report[89] != crate::packet::crc(&report[1..]))
                    {
                        bail!("verify: invalid query response status, transaction or checksum");
                    }
                    if resp.class != sent[7] || resp.cmd != sent[8] {
                        bail!(
                            "response does not echo the request (got class 0x{:02x} cmd 0x{:02x}, \
                             sent class 0x{:02x} cmd 0x{:02x})",
                            resp.class,
                            resp.cmd,
                            sent[7],
                            sent[8]
                        );
                    }
                    return Ok(resp);
                }
                status => bail!("device answered: {status}"),
            }
        }
        bail!("device still busy after {BUSY_RETRIES} retries")
    }
}

/// One row per HID interface of the pad, for `padctl list`.
pub fn list(api: &HidApi, selector: &Selector) -> Vec<String> {
    candidates(api, selector)
        .into_iter()
        .map(|d| {
            let role = if d.interface_number() == 0 {
                "control"
            } else {
                "events (unused)"
            };
            let serial = match d.serial_number() {
                Some(s) if !s.is_empty() => format!(", serial {s}"),
                _ => String::new(),
            };
            format!(
                "interface {} — usage page 0x{:04x}, usage 0x{:02x}{}, path {} [{}]",
                d.interface_number(),
                d.usage_page(),
                d.usage(),
                serial,
                d.path().to_string_lossy(),
                role,
            )
        })
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Scripted transport: records everything sent, pops pre-scripted reads.
    #[derive(Default)]
    struct Script {
        sent: Vec<[u8; REPORT_LEN]>,
        reads: VecDeque<[u8; REPORT_LEN]>,
    }

    #[derive(Clone, Default)]
    struct Mock(Rc<RefCell<Script>>);

    impl Transport for Mock {
        fn send_report(&self, report: &[u8; REPORT_LEN]) -> Result<()> {
            self.0.borrow_mut().sent.push(*report);
            Ok(())
        }

        fn read_report(&self) -> Result<[u8; REPORT_LEN]> {
            self.0
                .borrow_mut()
                .reads
                .pop_front()
                .context("mock: no more scripted reads")
        }
    }

    impl Mock {
        fn pad(&self, verify: bool) -> Pad {
            Pad::with_transport(Box::new(self.clone()), OpenOpts { verify })
        }

        fn script_read(&self, report: [u8; REPORT_LEN]) {
            self.0.borrow_mut().reads.push_back(report);
        }

        fn sent_count(&self) -> usize {
            self.0.borrow().sent.len()
        }
    }

    /// A device report echoing `packet`'s class/cmd with the given status.
    fn echo(packet: &Packet, status: u8) -> [u8; REPORT_LEN] {
        let mut report = packet.to_report();
        report[1] = status;
        report
    }

    #[test]
    fn query_returns_matching_response() {
        let mock = Mock::default();
        let q = rgb::brightness_get();
        let mut ok = echo(&q, 0x02);
        ok[11] = 0xFF; // brightness arg
        mock.script_read(ok);
        let resp = mock.pad(false).query(&q).unwrap();
        assert_eq!(resp.status, Status::Ok);
        assert_eq!(resp.args[2], 0xFF);
        assert_eq!(mock.sent_count(), 1);
    }

    #[test]
    fn query_retries_while_busy_then_succeeds() {
        let mock = Mock::default();
        let q = rgb::firmware_version();
        mock.script_read(echo(&q, 0x01)); // busy
        mock.script_read(echo(&q, 0x01)); // busy
        mock.script_read(echo(&q, 0x02)); // ok
        let resp = mock.pad(false).query(&q).unwrap();
        assert_eq!(resp.status, Status::Ok);
        assert_eq!(mock.sent_count(), 3); // the query is re-sent per attempt
    }

    #[test]
    fn query_gives_up_after_busy_retries() {
        let mock = Mock::default();
        let q = rgb::firmware_version();
        for _ in 0..=BUSY_RETRIES {
            mock.script_read(echo(&q, 0x01));
        }
        let err = mock.pad(false).query(&q).unwrap_err();
        assert!(err.to_string().contains("busy"), "{err}");
    }

    #[test]
    fn query_rejects_mismatched_echo() {
        let mock = Mock::default();
        mock.script_read(echo(&rgb::serial(), 0x02));
        let err = mock.pad(false).query(&rgb::firmware_version()).unwrap_err();
        assert!(err.to_string().contains("does not echo"), "{err}");
    }

    #[test]
    fn query_fails_on_error_status() {
        let mock = Mock::default();
        let q = rgb::firmware_version();
        mock.script_read(echo(&q, 0x05)); // not supported
        let err = mock.pad(false).query(&q).unwrap_err();
        assert!(err.to_string().contains("not supported"), "{err}");
    }

    #[test]
    fn send_without_verify_never_reads() {
        let mock = Mock::default();
        // No reads scripted: a read attempt would error.
        mock.pad(false).send(&rgb::spectrum()).unwrap();
        assert_eq!(mock.sent_count(), 1);
    }

    #[test]
    fn verify_accepts_ok_echo() {
        let mock = Mock::default();
        let cmd = rgb::spectrum();
        mock.script_read(echo(&cmd, 0x02));
        mock.pad(true).send(&cmd).unwrap();
    }

    #[test]
    fn status_light_requires_fan_ack_and_retries_changed_or_rejected_color() {
        use crate::lighting::{plan, sync_status};
        let config = crate::config::parse(include_str!("../config.mac-fans.toml")).unwrap();
        let mut plan = plan(&config.lighting).unwrap().unwrap();
        let mock = Mock::default();
        let pad = mock.pad(true);
        plan.connect(&pad).unwrap();
        assert_eq!(
            sync_status(Some(&mut plan), Some(&pad), None).unwrap(),
            None
        );
        assert_eq!(mock.sent_count(), 0);
        let brightness = rgb::brightness_set(89);
        let blue = rgb::live_solid(Some((0, 0, 255)));
        let amber = rgb::live_solid(Some((255, 160, 0)));
        let red = rgb::live_solid(Some((255, 0, 0)));
        let off = rgb::live_solid(None);
        let readback = |effect: &Packet| {
            let mut response = echo(&rgb::active_effect_get(), 2);
            response[9..18].copy_from_slice(&effect.to_report()[9..18]);
            response[89] = crate::packet::crc(&response[1..]);
            response
        };
        mock.script_read(echo(&brightness, 2));
        mock.script_read(echo(&blue, 2));
        mock.script_read(readback(&blue));
        let mut update = |rpm| sync_status(Some(&mut plan), Some(&pad), Some(rpm));
        assert_eq!(update(500).unwrap(), Some("blue"));
        assert_eq!(update(1450).unwrap(), Some("blue"));
        assert_eq!(mock.sent_count(), 3);
        // A successful write echo can coexist with a different active color.
        mock.script_read(echo(&amber, 2));
        mock.script_read(readback(&blue));
        assert!(update(1500).is_err());
        mock.script_read(echo(&brightness, 2));
        mock.script_read(echo(&amber, 3));
        assert!(update(1500).is_err());
        mock.script_read(echo(&brightness, 2));
        mock.script_read(echo(&amber, 2));
        mock.script_read(readback(&amber));
        assert_eq!(update(2950).unwrap(), Some("amber"));
        mock.script_read(echo(&red, 2));
        mock.script_read(readback(&red));
        assert_eq!(update(3000).unwrap(), Some("red"));
        mock.script_read(echo(&off, 2));
        mock.script_read(readback(&off));
        assert_eq!(update(0).unwrap(), Some("off"));
        plan.connect(&pad).unwrap();
        mock.script_read(echo(&brightness, 2));
        mock.script_read(echo(&off, 2));
        mock.script_read(readback(&off));
        assert_eq!(
            sync_status(Some(&mut plan), Some(&pad), Some(0)).unwrap(),
            Some("off")
        );
    }

    #[test]
    fn verify_retries_busy_echo() {
        let mock = Mock::default();
        let cmd = rgb::spectrum();
        mock.script_read(echo(&cmd, 0x01)); // busy
        mock.script_read(echo(&cmd, 0x02)); // then ok
        mock.pad(true).send(&cmd).unwrap();
    }

    #[test]
    fn verify_fails_on_rejected_command() {
        let mock = Mock::default();
        let cmd = rgb::spectrum();
        mock.script_read(echo(&cmd, 0x03)); // failure
        let err = mock.pad(true).send(&cmd).unwrap_err();
        assert!(err.to_string().contains("rejected"), "{err}");
    }

    #[test]
    fn verify_rejects_when_device_does_not_echo() {
        let mock = Mock::default();
        // Device answers with an unrelated report (e.g. the fan status).
        mock.script_read(echo(&crate::fan::off(), 0x02));
        assert!(mock.pad(true).send(&rgb::spectrum()).is_err());
    }
}
