//! Fan commands, byte-for-byte identical to the known-good FanControl
//! Windows plugin for this pad (see the FanControl plugin ecosystem at
//! <https://github.com/Rem0o/FanControl.Releases>): class 0x0D,
//! transaction id 0x02, speed encoded as RPM/50.

use crate::packet::{Packet, REPORT_LEN, TID_FAN};

pub const MIN_RPM: u32 = 500;
pub const MAX_RPM: u32 = 3200;
pub const RPM_STEP: u32 = 50;

const CLASS_FAN: u8 = 0x0D;

/// Clamp to the device's supported range and round to the 50 RPM step.
pub fn normalize_rpm(rpm: u32) -> u32 {
    let clamped = rpm.clamp(MIN_RPM, MAX_RPM);
    ((clamped + RPM_STEP / 2) / RPM_STEP) * RPM_STEP
}

/// Map 1-100% onto the supported RPM range (0% is handled as "off" upstream).
pub fn percent_to_rpm(pct: u32) -> u32 {
    normalize_rpm(MIN_RPM + (MAX_RPM - MIN_RPM) * pct.min(100) / 100)
}

/// Approximate position of an RPM within the supported range, for display.
pub fn rpm_to_percent(rpm: u32) -> u32 {
    let rpm = rpm.clamp(MIN_RPM, MAX_RPM);
    (rpm - MIN_RPM) * 100 / (MAX_RPM - MIN_RPM)
}

pub fn set_rpm(rpm: u32) -> Packet {
    let raw = (normalize_rpm(rpm) / RPM_STEP) as u16;
    Packet::new(
        TID_FAN,
        CLASS_FAN,
        0x01,
        0x03,
        &[0x01, 0x05, (raw & 0xFF) as u8, (raw >> 8) as u8],
    )
}

pub fn off() -> Packet {
    Packet::new(TID_FAN, CLASS_FAN, 0x10, 0x03, &[0x00, 0x06, 0x00, 0x00])
}

/// Decode an acknowledged fan command, not a measured tachometer speed.
/// Other queries replace the shared report buffer, making RPM unknown.
pub fn rpm_from_report(report: &[u8; REPORT_LEN]) -> Option<u32> {
    if report[0] != 0
        || report[1] != 2
        || report[2] != TID_FAN
        || report[7] != CLASS_FAN
        || report[89] != crate::packet::crc(&report[1..])
    {
        return None;
    }
    match (report[8], report[9], report[10]) {
        (0x10, 0, 6) => Some(0),
        (1, 1, 5) => {
            let rpm = (report[11] as u32 | (report[12] as u32) << 8) * RPM_STEP;
            (MIN_RPM..=MAX_RPM).contains(&rpm).then_some(rpm)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_lighting_response_and_unacknowledged_fan_command() {
        let mut report = crate::rgb::brightness_get().to_report();
        report[1] = 2;
        report[11] = 255;
        report[89] = crate::packet::crc(&report[1..]);
        assert_eq!(rpm_from_report(&report), None);
        let mut report = set_rpm(1500).to_report();
        assert_eq!(rpm_from_report(&report), None);
        report[1] = 2;
        assert_eq!(rpm_from_report(&report), Some(1500));
        report[11] ^= 1;
        assert_eq!(rpm_from_report(&report), None);
        let mut report = off().to_report();
        report[1] = 2;
        assert_eq!(rpm_from_report(&report), Some(0));
    }

    #[test]
    fn percent_round_trip_edges() {
        assert_eq!(percent_to_rpm(100), MAX_RPM);
        assert_eq!(percent_to_rpm(1), 550);
        assert_eq!(rpm_to_percent(MIN_RPM), 0);
        assert_eq!(rpm_to_percent(MAX_RPM), 100);
        assert_eq!(rpm_to_percent(1850), 50);
    }
}
