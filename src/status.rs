//! Atomic, read-only monitoring feed for desktop clients such as XRG.
use anyhow::{Context, Result};
use serde::Serialize;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub updated_at: f64,
    pub valid_for_seconds: u64,
    pub running: bool,
    pub connected: bool,
    pub fan_commanded_rpm: Option<u32>,
    pub lighting_status: Option<&'static str>,
    pub fan_measured_rpm: Option<u32>,
    pub pad_temperature_celsius: Option<f64>,
    pub mac_fans_rpm: Option<Vec<u32>>,
    pub die_celsius: Option<f64>,
    pub die_fahrenheit: Option<f64>,
    pub gpu_percent: Option<f64>,
    pub thermal_pressure: Option<&'static str>,
    pub error: Option<String>,
}

impl Snapshot {
    pub fn new(interval: u64) -> Self {
        Self {
            schema_version: 1,
            updated_at: 0.0,
            valid_for_seconds: interval.saturating_mul(3).clamp(15, 900),
            running: true,
            connected: false,
            fan_commanded_rpm: None,
            lighting_status: None,
            fan_measured_rpm: None,
            pad_temperature_celsius: None,
            mac_fans_rpm: None,
            die_celsius: None,
            die_fahrenheit: None,
            gpu_percent: None,
            thermal_pressure: None,
            error: None,
        }
    }

    pub fn set_temperature(&mut self, celsius: f64) {
        self.die_celsius = Some(celsius);
        self.die_fahrenheit = Some(crate::temp::units::Unit::Fahrenheit.convert(celsius));
    }
}

pub struct Publisher {
    path: PathBuf,
    _lock: std::fs::File,
}

impl Publisher {
    pub fn new() -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        let directory = if cfg!(target_os = "macos") {
            PathBuf::from(home).join("Library/Caches/padctl")
        } else {
            PathBuf::from(home).join(".cache/padctl")
        };
        std::fs::create_dir_all(&directory).context("creating monitoring directory")?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(directory.join("status.lock"))?;
        lock.try_lock()
            .context("another controller owns the monitoring feed")?;
        Ok(Self {
            path: directory.join("status.json"),
            _lock: lock,
        })
    }

    pub fn publish(&self, snapshot: &mut Snapshot) -> Result<()> {
        snapshot.updated_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
        let bytes = serde_json::to_vec(snapshot)?;
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .context("opening monitoring snapshot")?;
        file.write_all(&bytes)
            .context("writing monitoring snapshot")?;
        drop(file);
        std::fs::rename(&temporary, &self.path).context("publishing monitoring snapshot")
    }
}

/// Monitoring errors are reported without stopping the cooling controller.
pub fn publish(publisher: &Option<Publisher>, snapshot: &mut Snapshot) {
    if let Some(publisher) = publisher
        && let Err(error) = publisher.publish(snapshot)
    {
        log::error!("XRG monitoring snapshot failed: {error:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_device_measurements_are_null_not_zero() {
        let mut sample = Snapshot::new(2);
        sample.set_temperature(50.0);
        let json = serde_json::to_value(sample).unwrap();
        assert!(json["fan_measured_rpm"].is_null());
        assert!(json["fan_commanded_rpm"].is_null());
        assert!(json["pad_temperature_celsius"].is_null());
        assert_eq!(json["die_fahrenheit"], 122.0);
        assert_eq!(json["valid_for_seconds"], 15);
    }
}
