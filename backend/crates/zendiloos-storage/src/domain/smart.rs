/// What SMART says about a disk, or why it says nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum SmartReport {
    Available(Box<SmartData>),
    /// The disk is spun down; reading SMART would wake it, so it was not
    /// read.
    Standby,
    /// The device has no SMART, e.g. a virtual disk or a USB bridge that
    /// does not pass it through.
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartProtocol {
    Ata,
    Nvme,
    Scsi,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SmartData {
    pub protocol: SmartProtocol,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    /// The drive's own verdict; `None` if it gave none.
    pub passed: Option<bool>,
    pub temperature_celsius: Option<i64>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    /// ATA attributes; empty for other protocols.
    pub attributes: Vec<AtaAttribute>,
    pub nvme: Option<NvmeHealth>,
    /// Entries in the ATA error log.
    pub error_log_count: Option<u64>,
    /// The most recent self-test.
    pub last_self_test: Option<SelfTest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtaAttribute {
    pub id: u8,
    pub name: String,
    /// Normalized value; lower is worse.
    pub value: u16,
    pub worst: u16,
    pub threshold: u16,
    pub raw: u64,
    /// The raw value as smartctl formats it, e.g. `31 (Min/Max 22/45)`.
    pub raw_text: String,
    /// Failing this predicts imminent failure, as opposed to old age.
    pub prefailure: bool,
    pub failure: AttributeFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeFailure {
    None,
    /// At or below its threshold now.
    Now,
    /// Was at or below its threshold at some point.
    Past,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvmeHealth {
    /// Bit field; any bit set is a problem the drive reports.
    pub critical_warning: u8,
    /// Spare capacity left, in percent.
    pub available_spare: u8,
    pub available_spare_threshold: u8,
    /// Estimated share of the rated life used, in percent; may exceed 100.
    pub percentage_used: u8,
    pub media_errors: u64,
    pub unsafe_shutdowns: u64,
    /// In units of 512 000 bytes, as the specification defines them.
    pub data_units_read: u64,
    pub data_units_written: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfTest {
    pub passed: bool,
    /// How smartctl describes the result.
    pub description: String,
    pub power_on_hours: Option<u64>,
}

/// How worried to be about a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    Good,
    /// Signs of wear or past errors: back up and watch.
    Warning,
    /// Replace the disk.
    Failing,
}

/// A reason behind an [`Assessment`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// The drive itself reports that it is failing.
    OverallFailed,
    AttributeFailing {
        id: u8,
        name: String,
    },
    AttributeFailedInPast {
        id: u8,
        name: String,
    },
    ReallocatedSectors(u64),
    PendingSectors(u64),
    UncorrectableSectors(u64),
    ErrorLogEntries(u64),
    SelfTestFailed,
    NvmeCriticalWarning(u8),
    NvmeSpareLow {
        available: u8,
        threshold: u8,
    },
    NvmeWornOut(u8),
    NvmeMediaErrors(u64),
}

impl Finding {
    pub fn health(&self) -> Health {
        match self {
            Finding::OverallFailed
            | Finding::AttributeFailing { .. }
            | Finding::NvmeCriticalWarning(_)
            | Finding::NvmeSpareLow { .. } => Health::Failing,
            _ => Health::Warning,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub health: Health,
    pub findings: Vec<Finding>,
}

/// NVMe wear at which a drive counts as worn out, in percent of its rated
/// life.
const NVME_WORN_OUT: u8 = 90;

impl SmartData {
    /// Judges the data by rules that are common ground among SMART tools:
    /// the drive's own verdict, failing attributes, and the counters that
    /// predict failure (reallocated, pending and uncorrectable sectors).
    pub fn assess(&self) -> Assessment {
        let mut findings = Vec::new();
        if self.passed == Some(false) {
            findings.push(Finding::OverallFailed);
        }
        for attribute in &self.attributes {
            let named = || (attribute.id, attribute.name.clone());
            match attribute.failure {
                AttributeFailure::Now => {
                    let (id, name) = named();
                    findings.push(Finding::AttributeFailing { id, name });
                }
                AttributeFailure::Past => {
                    let (id, name) = named();
                    findings.push(Finding::AttributeFailedInPast { id, name });
                }
                AttributeFailure::None => {}
            }
            let count = attribute.raw;
            match attribute.id {
                5 if count > 0 => findings.push(Finding::ReallocatedSectors(count)),
                197 if count > 0 => findings.push(Finding::PendingSectors(count)),
                198 if count > 0 => findings.push(Finding::UncorrectableSectors(count)),
                _ => {}
            }
        }
        if let Some(count) = self.error_log_count.filter(|&count| count > 0) {
            findings.push(Finding::ErrorLogEntries(count));
        }
        if self.last_self_test.as_ref().is_some_and(|test| !test.passed) {
            findings.push(Finding::SelfTestFailed);
        }
        if let Some(nvme) = &self.nvme {
            if nvme.critical_warning != 0 {
                findings.push(Finding::NvmeCriticalWarning(nvme.critical_warning));
            }
            // Drives without a spare pool report 0 for both.
            if nvme.available_spare < nvme.available_spare_threshold {
                findings.push(Finding::NvmeSpareLow {
                    available: nvme.available_spare,
                    threshold: nvme.available_spare_threshold,
                });
            }
            if nvme.percentage_used >= NVME_WORN_OUT {
                findings.push(Finding::NvmeWornOut(nvme.percentage_used));
            }
            if nvme.media_errors > 0 {
                findings.push(Finding::NvmeMediaErrors(nvme.media_errors));
            }
        }
        let health = findings.iter().map(Finding::health).max().unwrap_or(Health::Good);
        Assessment { health, findings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ata(attributes: Vec<AtaAttribute>) -> SmartData {
        SmartData {
            protocol: SmartProtocol::Ata,
            model: None,
            serial: None,
            firmware: None,
            passed: Some(true),
            temperature_celsius: Some(31),
            power_on_hours: Some(1),
            power_cycles: Some(0),
            attributes,
            nvme: None,
            error_log_count: Some(0),
            last_self_test: None,
        }
    }

    fn attribute(id: u8, raw: u64, failure: AttributeFailure) -> AtaAttribute {
        AtaAttribute {
            id,
            name: format!("attribute {id}"),
            value: 100,
            worst: 100,
            threshold: 10,
            raw,
            raw_text: raw.to_string(),
            prefailure: true,
            failure,
        }
    }

    fn nvme(change: impl FnOnce(&mut NvmeHealth)) -> SmartData {
        let mut health = NvmeHealth {
            critical_warning: 0,
            available_spare: 100,
            available_spare_threshold: 10,
            percentage_used: 3,
            media_errors: 0,
            unsafe_shutdowns: 4,
            data_units_read: 1,
            data_units_written: 1,
        };
        change(&mut health);
        SmartData { protocol: SmartProtocol::Nvme, nvme: Some(health), error_log_count: None, ..ata(vec![]) }
    }

    #[test]
    fn a_healthy_disk_has_no_findings() {
        let data =
            ata(vec![attribute(5, 0, AttributeFailure::None), attribute(9, 4000, AttributeFailure::None)]);
        assert_eq!(data.assess(), Assessment { health: Health::Good, findings: vec![] });
        assert_eq!(nvme(|_| {}).assess().health, Health::Good);
        assert_eq!(
            nvme(|h| (h.available_spare, h.available_spare_threshold) = (0, 0)).assess().health,
            Health::Good
        );
    }

    #[test]
    fn sector_counters_are_warnings() {
        let data =
            ata(vec![attribute(5, 8, AttributeFailure::None), attribute(197, 1, AttributeFailure::None)]);
        let assessment = data.assess();
        assert_eq!(assessment.health, Health::Warning);
        assert_eq!(assessment.findings, [Finding::ReallocatedSectors(8), Finding::PendingSectors(1)]);
    }

    #[test]
    fn failures_now_outweigh_everything() {
        let data = SmartData {
            passed: Some(false),
            error_log_count: Some(3),
            ..ata(vec![attribute(1, 0, AttributeFailure::Now), attribute(7, 0, AttributeFailure::Past)])
        };
        let assessment = data.assess();
        assert_eq!(assessment.health, Health::Failing);
        assert_eq!(assessment.findings.len(), 4, "{:?}", assessment.findings);
    }

    #[test]
    fn nvme_rules() {
        assert_eq!(nvme(|h| h.critical_warning = 0x04).assess().health, Health::Failing);
        assert_eq!(nvme(|h| h.available_spare = 5).assess().health, Health::Failing);
        assert_eq!(nvme(|h| h.percentage_used = 95).assess().findings, [Finding::NvmeWornOut(95)]);
        assert_eq!(nvme(|h| h.media_errors = 2).assess().health, Health::Warning);
    }

    #[test]
    fn a_failed_self_test_is_a_warning() {
        let data = SmartData {
            last_self_test: Some(SelfTest {
                passed: false,
                description: "Completed: read failure".into(),
                power_on_hours: Some(100),
            }),
            ..ata(vec![])
        };
        assert_eq!(data.assess().findings, [Finding::SelfTestFailed]);
    }
}
