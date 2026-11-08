//! The built-in catalog of NAS models (docs/architecture.md, 5): a TOML
//! file each in `enclosure/`, embedded in the binary and checked when it
//! is read. A model's slots are bound to the ports of its bays.

use serde::Deserialize;
use zendiloos_storage::application::ports::EnclosureCatalog;
use zendiloos_storage::domain::{
    Binding, Chassis, ChassisStyle, DmiMatch, EnclosureModel, FrontPanel, Layout, Numbering, Orientation,
    PanelSide, Slot, SlotForm, SlotGroup,
};

/// Every model, by its file.
const MODELS: &[(&str, &str)] =
    &[("ugreen-dxp4800-plus", include_str!("enclosure/ugreen-dxp4800-plus.toml"))];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelFile {
    id: String,
    vendor: String,
    name: String,
    verified: bool,
    #[serde(default)]
    notes: String,
    dmi: DmiFile,
    chassis: ChassisFile,
    groups: Vec<GroupFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DmiFile {
    sys_vendor: String,
    product_name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChassisFile {
    style: String,
    panel: Option<PanelFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelFile {
    side: String,
    power_button: bool,
    #[serde(default)]
    leds: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupFile {
    id: String,
    name: String,
    rows: u8,
    columns: u8,
    orientation: String,
    form: String,
    numbering: String,
    slots: Vec<SlotFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotFile {
    id: String,
    label: String,
    port: Option<String>,
    serial: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("model {file} of the enclosure catalog: {reason}")]
pub struct CatalogError {
    file: &'static str,
    reason: String,
}

/// The models of the built-in catalog, read once.
#[derive(Debug, Clone)]
pub struct BuiltInCatalog {
    models: Vec<EnclosureModel>,
}

impl BuiltInCatalog {
    /// Reads and checks every model; tests make sure this never fails for
    /// the models shipped.
    pub fn load() -> Result<Self, CatalogError> {
        let models = MODELS.iter().map(|(file, text)| model(file, text)).collect::<Result<Vec<_>, _>>()?;
        for (index, model) in models.iter().enumerate() {
            if models[..index].iter().any(|other| other.id == model.id) {
                return Err(CatalogError { file: MODELS[index].0, reason: "its id is taken".into() });
            }
        }
        Ok(Self { models })
    }
}

impl EnclosureCatalog for BuiltInCatalog {
    fn models(&self) -> Vec<EnclosureModel> {
        self.models.clone()
    }
}

fn model(file: &'static str, text: &str) -> Result<EnclosureModel, CatalogError> {
    let error = |reason: String| CatalogError { file, reason };
    let parsed: ModelFile = toml::from_str(text).map_err(|e| error(e.to_string()))?;
    if parsed.id != file {
        return Err(error(format!("its id {:?} is not its file's name", parsed.id)));
    }
    let word = |what: &str, value: &str| error(format!("unknown {what} {value:?}"));
    let groups = parsed
        .groups
        .into_iter()
        .map(|group| {
            Ok(SlotGroup {
                orientation: Orientation::parse(&group.orientation)
                    .ok_or_else(|| word("orientation", &group.orientation))?,
                form: SlotForm::parse(&group.form).ok_or_else(|| word("form", &group.form))?,
                numbering: Numbering::parse(&group.numbering)
                    .ok_or_else(|| word("numbering", &group.numbering))?,
                slots: group
                    .slots
                    .into_iter()
                    .map(|slot| {
                        let binding = match (slot.port, slot.serial) {
                            (Some(port), None) => Some(Binding::Port(port)),
                            (None, Some(serial)) => Some(Binding::Serial(serial)),
                            (None, None) => None,
                            (Some(_), Some(_)) => {
                                return Err(error(format!("slot {} has two bindings", slot.id)));
                            }
                        };
                        Ok(Slot { id: slot.id, label: slot.label, binding })
                    })
                    .collect::<Result<_, _>>()?,
                id: group.id,
                name: group.name,
                rows: group.rows,
                columns: group.columns,
            })
        })
        .collect::<Result<Vec<_>, CatalogError>>()?;
    let panel = match parsed.chassis.panel {
        None => None,
        Some(panel) => Some(FrontPanel {
            side: PanelSide::parse(&panel.side).ok_or_else(|| word("side", &panel.side))?,
            power_button: panel.power_button,
            leds: panel.leds,
        }),
    };
    Ok(EnclosureModel {
        id: parsed.id,
        vendor: parsed.vendor,
        name: parsed.name,
        dmi: DmiMatch { vendor: parsed.dmi.sys_vendor, product: parsed.dmi.product_name },
        chassis: Chassis {
            style: ChassisStyle::parse(&parsed.chassis.style)
                .ok_or_else(|| word("style", &parsed.chassis.style))?,
            panel,
        },
        layout: Layout::new(groups).map_err(|e| error(e.to_string()))?,
        verified: parsed.verified,
        notes: parsed.notes.trim().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use zendiloos_storage::domain::{Machine, detect};

    use super::*;

    #[test]
    fn every_model_shipped_reads_and_the_dxp4800_plus_is_one() {
        let models = BuiltInCatalog::load().unwrap().models();
        let dxp = models.iter().find(|m| m.id == "ugreen-dxp4800-plus").unwrap();
        assert_eq!((dxp.vendor.as_str(), dxp.name.as_str()), ("UGREEN", "DXP4800 Plus"));
        assert!(!dxp.verified, "the bays' order is still to be confirmed");
        assert!(dxp.notes.starts_with("To confirm:"), "{}", dxp.notes);
        assert_eq!(dxp.chassis.style, ChassisStyle::Tower);
        assert_eq!(dxp.chassis.panel.as_ref().unwrap().side, PanelSide::Left);
        let ports: Vec<_> = dxp
            .layout
            .slots()
            .map(|slot| match &slot.binding {
                Some(Binding::Port(port)) => (slot.label.as_str(), port.as_str()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            ports,
            [
                ("Bay 1", "pci-0000:05:00.0-ata-1"),
                ("Bay 2", "pci-0000:05:00.0-ata-2"),
                ("Bay 3", "pci-0000:05:00.0-ata-3"),
                ("Bay 4", "pci-0000:05:00.0-ata-4"),
                ("M.2 1", "pci-0000:01:00.0-nvme-1"),
                ("M.2 2", "pci-0000:02:00.0-nvme-1"),
                ("System SSD", "pci-0000:04:00.0-nvme-1"),
            ]
        );
        let machine = Machine { vendor: "UGREEN\n".into(), product: "DXP4800 Plus\n".into() };
        assert_eq!(detect(&models, &machine).map(|m| m.id.as_str()), Some("ugreen-dxp4800-plus"));
        let qemu = Machine { vendor: "QEMU".into(), product: "Standard PC (Q35 + ICH9, 2009)".into() };
        assert!(detect(&models, &qemu).is_none());
    }

    const MINIMAL: &str = r#"
        id = "x"
        vendor = "V"
        name = "N"
        verified = true
        [dmi]
        sys_vendor = "V"
        product_name = "N"
        [chassis]
        style = "plain"
        [[groups]]
        id = "g"
        name = "G"
        rows = 1
        columns = 1
        orientation = "vertical"
        form = "inch_2_5"
        numbering = "left_to_right"
        slots = [{ id = "s", label = "S", serial = "A1" }]
    "#;

    #[test]
    fn a_model_file_is_checked_whole() {
        let minimal = model("x", MINIMAL).unwrap();
        assert_eq!(minimal.layout.slot("s").unwrap().binding, Some(Binding::Serial("A1".into())));
        assert!(minimal.chassis.panel.is_none() && minimal.notes.is_empty());
        let unbound = MINIMAL.replace(r#", serial = "A1""#, "");
        assert_eq!(model("x", &unbound).unwrap().layout.slot("s").unwrap().binding, None);

        let refused = |text: &str| model("x", text).unwrap_err().to_string();
        assert!(refused(&MINIMAL.replace(r#"id = "x""#, r#"id = "y""#)).contains("not its file's name"));
        assert!(refused(&MINIMAL.replace("plain", "blade")).contains("unknown style"));
        assert!(refused(&MINIMAL.replace(r#""vertical""#, r#""up""#)).contains("unknown orientation"));
        assert!(refused(&MINIMAL.replace("inch_2_5", "inch_5_25")).contains("unknown form"));
        assert!(refused(&MINIMAL.replace(r#""left_to_right""#, r#""spiral""#)).contains("unknown numbering"));
        assert!(
            refused(&MINIMAL.replace(r#"serial = "A1""#, r#"serial = "A1", port = "pci-1""#))
                .contains("two bindings")
        );
        assert!(refused(&MINIMAL.replace("rows = 1", "rows = 2")).contains("one slot for each place"));
        assert!(
            refused(&MINIMAL.replace("verified = true", "verified = true\ncolour = 1"))
                .contains("unknown field")
        );
        let panel = MINIMAL.replace(
            r#"style = "plain""#,
            "style = \"rack\"\n[chassis.panel]\nside = \"top\"\npower_button = false",
        );
        assert!(refused(&panel).contains("unknown side"));
        let error = CatalogError { file: "x", reason: "its id is taken".into() };
        assert_eq!(error.to_string(), "model x of the enclosure catalog: its id is taken");
    }
}
