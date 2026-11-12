/// What SMART says about a disk, or why it says nothing. An eMMC device,
/// which has no SMART, reports its wear instead ([`EmmcWear`]).
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
    /// Not SMART: what an eMMC device reports of its wear.
    Emmc,
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
    pub emmc: Option<EmmcWear>,
    /// Entries in the ATA error log.
    pub error_log_count: Option<u64>,
    /// The most recent self-test.
    pub last_self_test: Option<SelfTest>,
    /// A self-test the drive is running now.
    pub running_self_test: Option<RunningSelfTest>,
    /// How long the drive says its tests take, in minutes: short, long.
    pub self_test_minutes: Option<(u32, u32)>,
    /// Whether the drive can test itself at all.
    pub self_tests_supported: bool,
}

/// The tests a drive runs on itself, without stopping its work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SelfTestKind {
    /// A few minutes: electronics and a sample of the surface.
    Short,
    /// Hours: every sector. Run it when the disk is little used.
    Long,
}

impl SelfTestKind {
    pub const ALL: [SelfTestKind; 2] = [Self::Short, Self::Long];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Long => "long",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunningSelfTest {
    /// What is left, 0 to 100, if the drive says.
    pub remaining_percent: Option<u8>,
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

/// What an eMMC device of version 5.0 or later reports of its wear, from
/// its EXT_CSD register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmmcWear {
    /// The device's estimates of the share of its life used, for its two
    /// kinds of memory cells (A and B, often its SLC and its MLC part), in
    /// steps of ten percent: 1 is 0 to 10 % used, 10 is 90 to 100 %, 11
    /// past its life. `None` where it gives no estimate.
    pub life_used_a: Option<u8>,
    pub life_used_b: Option<u8>,
    /// How far its reserved blocks, which replace worn ones, are used.
    pub pre_eol: Option<PreEol>,
}

impl EmmcWear {
    /// The step past which the device's life is over.
    pub const PAST_LIFE: u8 = 11;

    /// The higher of the two estimates.
    pub fn life_used(&self) -> Option<u8> {
        self.life_used_a.max(self.life_used_b)
    }
}

/// How far an eMMC device's reserved blocks are used (`PRE_EOL_INFO`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PreEol {
    Normal,
    /// 80 % of them used.
    Warning,
    /// 90 % of them used.
    Urgent,
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
    /// An eMMC device estimates that it used at least this share of its
    /// life, in percent ([`EMMC_WORN`] or more).
    EmmcWornOut(u8),
    /// An eMMC device is past the life it was made for.
    EmmcPastLife,
    /// An eMMC device used most of its reserved blocks: 80 % (a warning) or
    /// 90 % (urgent).
    EmmcReserveLow(PreEol),
}

impl Finding {
    pub fn health(&self) -> Health {
        match self {
            Finding::OverallFailed
            | Finding::AttributeFailing { .. }
            | Finding::NvmeCriticalWarning(_)
            | Finding::NvmeSpareLow { .. }
            | Finding::EmmcPastLife
            | Finding::EmmcReserveLow(PreEol::Urgent) => Health::Failing,
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

/// eMMC wear at which a device counts as worn out, in percent of its life:
/// earlier than an NVMe drive, since eMMC is soldered to the board and the
/// system has to move to another disk before it gives out.
pub const EMMC_WORN: u8 = 80;

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
        if let Some(emmc) = &self.emmc {
            match emmc.life_used() {
                Some(step) if step >= EmmcWear::PAST_LIFE => findings.push(Finding::EmmcPastLife),
                // Step n: at least (n - 1) × 10 % used.
                Some(step) if step.saturating_sub(1) * 10 >= EMMC_WORN => {
                    findings.push(Finding::EmmcWornOut(step.saturating_sub(1) * 10));
                }
                _ => {}
            }
            if let Some(pre_eol @ (PreEol::Warning | PreEol::Urgent)) = emmc.pre_eol {
                findings.push(Finding::EmmcReserveLow(pre_eol));
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
            emmc: None,
            error_log_count: Some(0),
            last_self_test: None,
            running_self_test: None,
            self_test_minutes: None,
            self_tests_supported: true,
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

    fn emmc(life_used_a: u8, life_used_b: u8, pre_eol: PreEol) -> SmartData {
        let wear = EmmcWear {
            life_used_a: Some(life_used_a),
            life_used_b: Some(life_used_b),
            pre_eol: Some(pre_eol),
        };
        SmartData { protocol: SmartProtocol::Emmc, emmc: Some(wear), passed: None, ..ata(vec![]) }
    }

    #[test]
    fn emmc_rules() {
        assert_eq!(
            emmc(1, 2, PreEol::Normal).assess(),
            Assessment { health: Health::Good, findings: vec![] }
        );
        assert_eq!(emmc(8, 3, PreEol::Normal).assess().health, Health::Good, "70 to 80 % used");
        assert_eq!(
            emmc(2, 9, PreEol::Normal).assess().findings,
            [Finding::EmmcWornOut(80)],
            "the higher counts"
        );
        assert_eq!(emmc(10, 1, PreEol::Normal).assess().health, Health::Warning);
        assert_eq!(emmc(11, 1, PreEol::Normal).assess().findings, [Finding::EmmcPastLife]);
        assert_eq!(emmc(11, 1, PreEol::Normal).assess().health, Health::Failing);
        let reserve = emmc(3, 3, PreEol::Warning).assess();
        assert_eq!(
            (reserve.health, reserve.findings),
            (Health::Warning, vec![Finding::EmmcReserveLow(PreEol::Warning)])
        );
        assert_eq!(emmc(3, 3, PreEol::Urgent).assess().health, Health::Failing);
        let unknown = SmartData {
            emmc: Some(EmmcWear { life_used_a: None, life_used_b: None, pre_eol: None }),
            ..ata(vec![])
        };
        assert_eq!(unknown.assess().health, Health::Good);
    }

    #[test]
    fn a_failed_self_test_is_a_warning() {
        let data = SmartData {
            last_self_test: Some(SelfTest {
                passed: false,
                description: "Completed: read failure".into(),
                power_on_hours: Some(100),
            }),
            running_self_test: None,
            self_test_minutes: None,
            self_tests_supported: true,
            ..ata(vec![])
        };
        assert_eq!(data.assess().findings, [Finding::SelfTestFailed]);
    }
}
