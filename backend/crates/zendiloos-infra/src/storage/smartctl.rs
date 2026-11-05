//! `smartctl --json=c --all --nocheck=standby` into a SMART report.
//!
//! The JSON layout is smartctl's (7.x, `json_format_version` 1). Only the
//! fields the Storage context uses are read; all of them are optional,
//! because what a drive reports varies widely.

use serde::Deserialize;
use zendiloos_storage::domain::{
    AtaAttribute, AttributeFailure, NvmeHealth, RunningSelfTest, SelfTest, SmartData, SmartProtocol,
    SmartReport,
};

use super::ParseError;

/// Bit 1 of the exit status: the device could not be opened, or it is in
/// standby and was left alone.
const DEVICE_NOT_READ: i32 = 1 << 1;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Output {
    smartctl: Option<Program>,
    device: Option<Device>,
    model_name: Option<String>,
    serial_number: Option<String>,
    firmware_version: Option<String>,
    smart_support: Option<Support>,
    smart_status: Option<Status>,
    temperature: Option<Temperature>,
    power_on_time: Option<PowerOnTime>,
    power_cycle_count: Option<u64>,
    ata_smart_attributes: Option<Attributes>,
    ata_smart_error_log: Option<ErrorLog>,
    ata_smart_self_test_log: Option<SelfTestLog>,
    ata_smart_data: Option<AtaSmartData>,
    nvme_smart_health_information_log: Option<NvmeLog>,
    nvme_self_test_log: Option<NvmeSelfTestLog>,
}

#[derive(Deserialize)]
struct AtaSmartData {
    self_test: Option<AtaSelfTest>,
    capabilities: Option<AtaCapabilities>,
}

#[derive(Deserialize)]
struct AtaCapabilities {
    self_tests_supported: Option<bool>,
}

#[derive(Deserialize)]
struct AtaSelfTest {
    status: Option<AtaSelfTestStatus>,
    polling_minutes: Option<PollingMinutes>,
}

#[derive(Deserialize)]
struct AtaSelfTestStatus {
    /// 241 to 249 while a test runs.
    value: u8,
    remaining_percent: Option<u8>,
}

#[derive(Deserialize)]
struct PollingMinutes {
    short: Option<u32>,
    extended: Option<u32>,
}

/// NVMe's own self-test log (smartctl 7.3 and later).
#[derive(Deserialize)]
struct NvmeSelfTestLog {
    current_self_test_operation: Option<Coded>,
    current_self_test_completion_percent: Option<u8>,
    /// Newest first.
    #[serde(default)]
    table: Vec<NvmeSelfTestEntry>,
}

#[derive(Deserialize)]
struct Coded {
    value: u8,
    #[serde(default)]
    string: String,
}

#[derive(Deserialize)]
struct NvmeSelfTestEntry {
    self_test_code: Option<Coded>,
    self_test_result: Coded,
    power_on_hours: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Program {
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Message {
    string: String,
}

#[derive(Deserialize)]
struct Device {
    protocol: String,
}

#[derive(Deserialize)]
struct Support {
    available: bool,
    #[serde(default = "yes")]
    enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct Status {
    passed: Option<bool>,
}

#[derive(Deserialize)]
struct Temperature {
    current: Option<i64>,
}

#[derive(Deserialize)]
struct PowerOnTime {
    hours: Option<u64>,
}

#[derive(Deserialize)]
struct Attributes {
    table: Vec<Attribute>,
}

#[derive(Deserialize)]
struct Attribute {
    id: u8,
    name: String,
    value: u16,
    worst: u16,
    thresh: u16,
    #[serde(default)]
    when_failed: String,
    flags: AttributeFlags,
    raw: Raw,
}

#[derive(Deserialize)]
struct AttributeFlags {
    prefailure: bool,
}

#[derive(Deserialize)]
struct Raw {
    value: u64,
    string: String,
}

#[derive(Deserialize)]
struct ErrorLog {
    summary: Option<Count>,
}

#[derive(Deserialize)]
struct Count {
    count: u64,
}

#[derive(Deserialize)]
struct SelfTestLog {
    standard: Option<SelfTestTable>,
}

#[derive(Deserialize)]
struct SelfTestTable {
    /// Newest first.
    #[serde(default)]
    table: Vec<SelfTestEntry>,
}

#[derive(Deserialize)]
struct SelfTestEntry {
    status: SelfTestStatus,
    lifetime_hours: Option<u64>,
}

#[derive(Deserialize)]
struct SelfTestStatus {
    string: String,
    #[serde(default = "yes")]
    passed: bool,
}

#[derive(Deserialize)]
struct NvmeLog {
    critical_warning: u8,
    available_spare: u8,
    available_spare_threshold: u8,
    percentage_used: u8,
    media_errors: u64,
    unsafe_shutdowns: u64,
    data_units_read: u64,
    data_units_written: u64,
}

pub fn parse(exit_status: i32, json: &str) -> Result<SmartReport, ParseError> {
    let output: Output = serde_json::from_str(json).map_err(|e| ParseError::new("smartctl", e))?;
    let messages: Vec<String> =
        output.smartctl.unwrap_or_default().messages.into_iter().map(|m| m.string).collect();

    // smartctl reports skipping a sleeping disk only as a message, e.g.
    // "Device is in STANDBY mode, exit(2)".
    let sleeping = messages.iter().any(|m| m.contains("STANDBY") || m.contains("SLEEP"));
    if exit_status & DEVICE_NOT_READ != 0 && sleeping {
        return Ok(SmartReport::Standby);
    }
    let unsupported = |fallback: &str| SmartReport::Unsupported {
        reason: messages.first().cloned().unwrap_or_else(|| fallback.to_owned()),
    };
    let Some(device) = output.device else { return Ok(unsupported("unknown device type")) };
    match output.smart_support {
        Some(Support { available: false, .. }) => return Ok(unsupported("the device has no SMART")),
        Some(Support { enabled: false, .. }) => {
            return Ok(SmartReport::Unsupported { reason: "SMART is disabled on the device".to_owned() });
        }
        _ => {}
    }
    let protocol = match device.protocol.as_str() {
        "ATA" => SmartProtocol::Ata,
        "NVMe" => SmartProtocol::Nvme,
        "SCSI" => SmartProtocol::Scsi,
        _ => return Ok(unsupported("unknown protocol")),
    };
    if output.smart_status.is_none()
        && output.ata_smart_attributes.is_none()
        && output.nvme_smart_health_information_log.is_none()
    {
        return Ok(unsupported("the device returned no SMART data"));
    }

    let attributes = output
        .ata_smart_attributes
        .map(|attributes| attributes.table)
        .unwrap_or_default()
        .into_iter()
        .map(|a| AtaAttribute {
            id: a.id,
            name: a.name,
            value: a.value,
            worst: a.worst,
            threshold: a.thresh,
            raw: a.raw.value,
            raw_text: a.raw.string,
            prefailure: a.flags.prefailure,
            failure: match a.when_failed.as_str() {
                "now" => AttributeFailure::Now,
                "past" => AttributeFailure::Past,
                _ => AttributeFailure::None,
            },
        })
        .collect();
    let nvme = output.nvme_smart_health_information_log.map(|log| NvmeHealth {
        critical_warning: log.critical_warning,
        available_spare: log.available_spare,
        available_spare_threshold: log.available_spare_threshold,
        percentage_used: log.percentage_used,
        media_errors: log.media_errors,
        unsafe_shutdowns: log.unsafe_shutdowns,
        data_units_read: log.data_units_read,
        data_units_written: log.data_units_written,
    });
    let (ata_test, ata_self_tests) = match output.ata_smart_data {
        Some(data) => {
            (data.self_test, data.capabilities.and_then(|c| c.self_tests_supported).unwrap_or(false))
        }
        None => (None, false),
    };
    let mut running_self_test = ata_test
        .as_ref()
        .and_then(|test| test.status.as_ref())
        .filter(|status| (241..=249).contains(&status.value))
        .map(|status| RunningSelfTest { remaining_percent: status.remaining_percent });
    let self_test_minutes = ata_test
        .and_then(|test| test.polling_minutes)
        .and_then(|minutes| Some((minutes.short?, minutes.extended?)));
    let mut last_self_test = output
        .ata_smart_self_test_log
        .and_then(|log| log.standard)
        .and_then(|log| log.table.into_iter().next())
        .map(|entry| SelfTest {
            passed: entry.status.passed,
            description: entry.status.string,
            power_on_hours: entry.lifetime_hours,
        });
    // smartctl reads an NVMe drive's self-test log only if it can test itself.
    let self_tests_supported = ata_self_tests || output.nvme_self_test_log.is_some();
    if let Some(log) = output.nvme_self_test_log {
        // 0: no test running; the completion is the share done.
        if log.current_self_test_operation.is_some_and(|operation| operation.value != 0) {
            running_self_test = Some(RunningSelfTest {
                remaining_percent: log.current_self_test_completion_percent.map(|done| 100 - done.min(100)),
            });
        }
        if let Some(entry) = log.table.into_iter().next() {
            let kind = entry.self_test_code.map(|code| code.string).filter(|kind| !kind.is_empty());
            last_self_test = Some(SelfTest {
                // Result 0: completed without error; 15 marks an unused entry.
                passed: entry.self_test_result.value == 0,
                description: match kind {
                    Some(kind) => format!("{kind}: {}", entry.self_test_result.string),
                    None => entry.self_test_result.string,
                },
                power_on_hours: entry.power_on_hours,
            });
        }
    }

    Ok(SmartReport::Available(Box::new(SmartData {
        protocol,
        model: output.model_name,
        serial: output.serial_number,
        firmware: output.firmware_version,
        passed: output.smart_status.and_then(|status| status.passed),
        temperature_celsius: output.temperature.and_then(|t| t.current),
        power_on_hours: output.power_on_time.and_then(|t| t.hours),
        power_cycles: output.power_cycle_count,
        attributes,
        nvme,
        error_log_count: output.ata_smart_error_log.and_then(|log| log.summary).map(|s| s.count),
        last_self_test,
        running_self_test,
        self_test_minutes,
        self_tests_supported,
    })))
}

#[cfg(test)]
mod tests {
    use zendiloos_storage::domain::Health;

    use super::*;

    /// `smartctl --json=c --all` in the test VM right after
    /// `smartctl -t short`: QEMU runs the test at once and logs it.
    #[test]
    fn reads_the_self_test_log_and_how_long_tests_take() {
        let SmartReport::Available(data) =
            parse(0, include_str!("fixtures/smartctl-sata-qemu-self-tested.json")).unwrap()
        else {
            panic!("no SMART data")
        };
        let last = data.last_self_test.unwrap();
        assert!(last.passed);
        assert_eq!(last.description, "Completed without error");
        assert_eq!(data.running_self_test, None);
        assert_eq!(data.self_test_minutes, Some((2, 54)));
        assert!(data.self_tests_supported);
    }

    /// QEMU's NVMe disk cannot test itself; `smartctl -t short` then says so
    /// and still exits with 0, so whether it can is read beforehand: an
    /// NVMe drive that can has a self-test log in `--all`.
    #[test]
    fn knows_which_drives_can_test_themselves() {
        let nvme = available(parse(0, include_str!("fixtures/smartctl-nvme-qemu.json")).unwrap());
        assert!(!nvme.self_tests_supported);
        let sata = available(parse(0, include_str!("fixtures/smartctl-sata-qemu.json")).unwrap());
        assert!(sata.self_tests_supported);
    }

    /// The forms smartctl documents for a test that runs (ATA status 241 to
    /// 249) and for NVMe's log; the test VM's devices show neither.
    #[test]
    fn reads_a_running_test_and_nvme_results() {
        let ata = r#"{"device":{"protocol":"ATA"},"smart_support":{"available":true,"enabled":true},"smart_status":{"passed":true},
            "ata_smart_data":{"self_test":{"status":{"value":249,"string":"in progress, 90% remaining","remaining_percent":90}}}}"#;
        let SmartReport::Available(data) = parse(0, ata).unwrap() else { panic!("no SMART data") };
        assert_eq!(data.running_self_test, Some(RunningSelfTest { remaining_percent: Some(90) }));

        let nvme = r#"{"device":{"protocol":"NVMe"},"smart_support":{"available":true,"enabled":true},"smart_status":{"passed":true},
            "nvme_self_test_log":{"current_self_test_operation":{"value":1,"string":"Short self-test in progress"},
            "current_self_test_completion_percent":30,
            "table":[{"self_test_code":{"value":2,"string":"Extended"},"self_test_result":{"value":7,"string":"Completed: failed segments"},"power_on_hours":800}]}}"#;
        let SmartReport::Available(data) = parse(0, nvme).unwrap() else { panic!("no SMART data") };
        assert_eq!(data.running_self_test, Some(RunningSelfTest { remaining_percent: Some(70) }));
        assert!(data.self_tests_supported);
        let last = data.last_self_test.unwrap();
        assert!(!last.passed, "a failed NVMe test is not read as passed");
        assert_eq!(last.description, "Extended: Completed: failed segments");
    }

    fn available(report: SmartReport) -> SmartData {
        match report {
            SmartReport::Available(data) => *data,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_sata_disk() {
        let data = available(parse(0, include_str!("fixtures/smartctl-sata-qemu.json")).unwrap());
        assert_eq!(data.protocol, SmartProtocol::Ata);
        assert_eq!(data.model.as_deref(), Some("QEMU HARDDISK"));
        assert_eq!(data.passed, Some(true));
        assert_eq!(data.temperature_celsius, Some(31));
        assert_eq!(data.power_on_hours, Some(1));
        assert_eq!(data.error_log_count, Some(0));
        assert_eq!(data.last_self_test, None);
        let reallocated = data.attributes.iter().find(|a| a.id == 5).unwrap();
        assert_eq!((reallocated.name.as_str(), reallocated.raw), ("Reallocated_Sector_Ct", 0));
        assert_eq!(reallocated.threshold, 36);
        assert!(reallocated.prefailure);
        assert_eq!(data.attributes.len(), 7);
        assert_eq!(data.assess().health, Health::Good);
    }

    #[test]
    fn an_nvme_disk() {
        let data = available(parse(0, include_str!("fixtures/smartctl-nvme-qemu.json")).unwrap());
        assert_eq!(data.protocol, SmartProtocol::Nvme);
        assert_eq!(data.temperature_celsius, Some(50));
        assert!(data.attributes.is_empty());
        let nvme = data.nvme.unwrap();
        assert_eq!((nvme.available_spare, nvme.percentage_used, nvme.media_errors), (0, 0, 0));
        assert_eq!(data.assess().health, Health::Good);
    }

    #[test]
    fn a_virtual_disk_without_smart() {
        let report = parse(1, include_str!("fixtures/smartctl-virtio-unsupported.json")).unwrap();
        assert_eq!(
            report,
            SmartReport::Unsupported { reason: "/dev/vda: Unable to detect device type".into() }
        );
    }

    /// Written after smartctl's behaviour, not captured: the VM's disks
    /// never sleep.
    #[test]
    fn a_sleeping_disk_is_left_alone() {
        let json = r#"{"smartctl": {"messages": [{"string": "Device is in STANDBY mode, exit(2)",
            "severity": "information"}], "exit_status": 2}, "device": {"protocol": "ATA"}}"#;
        assert_eq!(parse(2, json).unwrap(), SmartReport::Standby);
    }

    #[test]
    fn findings_come_through() {
        let json = r#"{"device": {"protocol": "ATA"}, "smart_status": {"passed": false},
            "ata_smart_attributes": {"table": [{"id": 5, "name": "Reallocated_Sector_Ct",
                "value": 1, "worst": 1, "thresh": 36, "when_failed": "now",
                "flags": {"prefailure": true}, "raw": {"value": 4000, "string": "4000"}}]},
            "ata_smart_self_test_log": {"standard": {"count": 1, "table": [{"type": {"value": 1},
                "status": {"value": 121, "string": "Completed: read failure", "passed": false},
                "lifetime_hours": 9000}]}}}"#;
        let data = available(parse(8 | 16 | 128, json).unwrap());
        assert_eq!(data.last_self_test.as_ref().unwrap().description, "Completed: read failure");
        let assessment = data.assess();
        assert_eq!(assessment.health, Health::Failing);
        assert_eq!(assessment.findings.len(), 4, "{:?}", assessment.findings);
    }

    #[test]
    fn refuses_what_is_not_json() {
        assert!(parse(1, "smartctl: command not found").is_err());
    }
}
