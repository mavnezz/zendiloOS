//! What an eMMC device's card says in sysfs (`/sys/class/block/<disk>/device`)
//! into a report in SMART's place: its two estimates of the life used and
//! how far its reserved blocks are used. eMMC 5.0 (JESD84-B50) defines them
//! in EXT_CSD, and the kernel prints them as hexadecimal bytes
//! (`drivers/mmc/core/mmc.c`):
//!
//! - `life_time`, `DEVICE_LIFE_TIME_EST_TYP_A` and `_B`: `0x01` is 0 to
//!   10 % of the life used, `0x0a` 90 to 100 %, `0x0b` past it;
//! - `pre_eol_info`, `PRE_EOL_INFO`: `0x01` normal, `0x02` a warning (80 %
//!   of the reserved blocks used), `0x03` urgent (90 %);
//! - `0x00` in either: not defined, as on devices older than 5.0.
//!
//! The fixtures are laid out as the kernel's files, in its format.

use zendiloos_storage::domain::{EmmcWear, PreEol, SmartData, SmartProtocol, SmartReport};

/// The card's files, as the agent read them (empty where there is none).
pub(crate) struct Card<'a> {
    pub kind: &'a str,
    pub life_time: &'a str,
    pub pre_eol_info: &'a str,
    pub name: &'a str,
    pub serial: &'a str,
    pub fwrev: &'a str,
}

/// One of the kernel's hexadecimal bytes, `0x0a`.
fn byte(text: &str) -> Option<u8> {
    u8::from_str_radix(text.trim().strip_prefix("0x")?, 16).ok()
}

/// A step of the life used, 1 to 11; 0 and anything else is no estimate.
fn life_step(text: &str) -> Option<u8> {
    byte(text).filter(|step| (1..=EmmcWear::PAST_LIFE).contains(step))
}

fn text(value: &str) -> Option<String> {
    Some(value.trim().to_owned()).filter(|value| !value.is_empty())
}

pub(crate) fn parse(card: &Card<'_>) -> SmartReport {
    if card.kind.trim() != "MMC" {
        let reason = match card.kind.trim() {
            "SD" => "a memory card reports no wear".to_owned(),
            "" => "the device says nothing of its wear".to_owned(),
            other => format!("a device of the kind {other} reports no wear"),
        };
        return SmartReport::Unsupported { reason };
    }
    let mut estimates = card.life_time.split_whitespace();
    let (life_used_a, life_used_b) =
        (estimates.next().and_then(life_step), estimates.next().and_then(life_step));
    let pre_eol = match byte(card.pre_eol_info) {
        Some(1) => Some(PreEol::Normal),
        Some(2) => Some(PreEol::Warning),
        Some(3) => Some(PreEol::Urgent),
        _ => None,
    };
    if (life_used_a, life_used_b, pre_eol) == (None, None, None) {
        return SmartReport::Unsupported {
            reason: "this eMMC reports no wear; devices older than version 5.0 do not".to_owned(),
        };
    }
    SmartReport::Available(Box::new(SmartData {
        protocol: SmartProtocol::Emmc,
        model: text(card.name),
        serial: text(card.serial),
        firmware: text(card.fwrev),
        passed: None,
        temperature_celsius: None,
        power_on_hours: None,
        power_cycles: None,
        attributes: Vec::new(),
        nvme: None,
        emmc: Some(EmmcWear { life_used_a, life_used_b, pre_eol }),
        error_log_count: None,
        last_self_test: None,
        running_self_test: None,
        self_test_minutes: None,
        self_tests_supported: false,
    }))
}

#[cfg(test)]
mod tests {
    use zendiloos_storage::domain::{Finding, Health};

    use super::*;

    /// The files of a fixture's card, read as the agent reads them.
    fn fixture(name: &str) -> SmartReport {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/storage/fixtures").join(name);
        let file = |file: &str| std::fs::read_to_string(dir.join(file)).unwrap_or_default();
        let (kind, life_time, pre_eol_info) = (file("type"), file("life_time"), file("pre_eol_info"));
        let (name, serial, fwrev) = (file("name"), file("serial"), file("fwrev"));
        parse(&Card {
            kind: &kind,
            life_time: &life_time,
            pre_eol_info: &pre_eol_info,
            name: &name,
            serial: &serial,
            fwrev: &fwrev,
        })
    }

    fn wear(report: &SmartReport) -> (EmmcWear, Health, Vec<Finding>) {
        let SmartReport::Available(data) = report else { panic!("no data: {report:?}") };
        let assessment = data.assess();
        (data.emmc.unwrap(), assessment.health, assessment.findings)
    }

    #[test]
    fn a_new_emmc_reports_its_wear_and_is_fine() {
        let report = fixture("emmc-5.1-new");
        let SmartReport::Available(data) = &report else { panic!("{report:?}") };
        assert_eq!(data.protocol, SmartProtocol::Emmc);
        assert_eq!(
            (data.model.as_deref(), data.serial.as_deref(), data.firmware.as_deref()),
            (Some("DV4032"), Some("0x5a1c3b2e"), Some("0x0300000000000000"))
        );
        assert!(!data.self_tests_supported, "eMMC cannot test itself");
        let (wear, health, findings) = wear(&report);
        assert_eq!(
            wear,
            EmmcWear { life_used_a: Some(1), life_used_b: Some(2), pre_eol: Some(PreEol::Normal) }
        );
        assert_eq!((health, findings), (Health::Good, vec![]));
    }

    #[test]
    fn a_worn_emmc_warns_and_one_past_its_life_is_failing() {
        let (wear_, health, findings) = wear(&fixture("emmc-5.1-worn"));
        assert_eq!(
            (wear_.life_used_a, wear_.life_used_b, wear_.pre_eol),
            (Some(9), Some(10), Some(PreEol::Warning))
        );
        assert_eq!(health, Health::Warning);
        assert_eq!(findings, [Finding::EmmcWornOut(90), Finding::EmmcReserveLow(PreEol::Warning)]);

        let (_, health, findings) = wear(&fixture("emmc-5.1-past-life"));
        assert_eq!(health, Health::Failing);
        assert_eq!(findings, [Finding::EmmcPastLife, Finding::EmmcReserveLow(PreEol::Urgent)]);
    }

    #[test]
    fn an_older_emmc_and_a_memory_card_report_no_wear() {
        for (name, says) in [("emmc-4.5", "older than version 5.0"), ("sd-card", "a memory card")] {
            let report = fixture(name);
            assert!(
                matches!(&report, SmartReport::Unsupported { reason } if reason.contains(says)),
                "{report:?}"
            );
        }
        let nothing =
            parse(&Card { kind: "", life_time: "", pre_eol_info: "", name: "", serial: "", fwrev: "" });
        assert!(matches!(nothing, SmartReport::Unsupported { .. }));
    }

    #[test]
    fn values_out_of_range_count_as_no_estimate() {
        let card = Card {
            kind: "MMC\n",
            life_time: "0x0c 0x03\n",
            pre_eol_info: "0x07\n",
            name: "",
            serial: "",
            fwrev: "",
        };
        let (wear, _, _) = wear(&parse(&card));
        assert_eq!(wear, EmmcWear { life_used_a: None, life_used_b: Some(3), pre_eol: None });
        assert_eq!((byte("0x"), byte("11"), byte(" 0x0A\n")), (None, None, Some(10)));
    }
}
