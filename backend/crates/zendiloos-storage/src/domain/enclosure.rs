//! Where each disk sits: the enclosure as groups of slots, what is bound
//! to each slot, the models of the built-in catalog, and which disk is in
//! which slot.
//!
//! A slot is bound to a port (where a disk is plugged in, udev's
//! `ID_PATH`), so a disk replaced in that slot shows up there by itself;
//! or to a serial number, for ports that are not stable (USB, some HBAs).

use super::disk::{Disk, serial_key};

/// Most groups in one enclosure.
pub const MAX_GROUPS: usize = 16;
/// Most slots in one group: rows × columns.
pub const MAX_GROUP_SLOTS: usize = 60;
pub const MAX_LABEL_CHARS: usize = 32;
pub const MAX_NAME_CHARS: usize = 64;
/// Ids of models, groups and slots: lowercase letters, digits and `-`.
pub const MAX_ID_CHARS: usize = 32;
const MAX_PORT_CHARS: usize = 128;
const MAX_SERIAL_CHARS: usize = 64;

/// How the bays of a group stand: side by side and upright, as at the
/// front of a tower NAS, or on top of each other and lying, as in a rack
/// or a cage of 2.5" bays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

impl Orientation {
    pub const ALL: [Orientation; 2] = [Self::Vertical, Self::Horizontal];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|orientation| orientation.as_str() == value)
    }
}

/// What fits into a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotForm {
    /// 3.5" drives.
    Inch35,
    /// 2.5" drives.
    Inch25,
    M2,
}

impl SlotForm {
    pub const ALL: [SlotForm; 3] = [Self::Inch35, Self::Inch25, Self::M2];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inch35 => "inch_3_5",
            Self::Inch25 => "inch_2_5",
            Self::M2 => "m2",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|form| form.as_str() == value)
    }
}

/// In which order a group's slots fill its grid: along the rows (from the
/// top row) or down the columns (from the left column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Numbering {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}

impl Numbering {
    pub const ALL: [Numbering; 4] =
        [Self::LeftToRight, Self::RightToLeft, Self::TopToBottom, Self::BottomToTop];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::LeftToRight => "left_to_right",
            Self::RightToLeft => "right_to_left",
            Self::TopToBottom => "top_to_bottom",
            Self::BottomToTop => "bottom_to_top",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|numbering| numbering.as_str() == value)
    }

    /// Row and column, from 0 at the top left, of the slot at `index` in a
    /// grid of `rows` × `columns`.
    pub fn cell(self, index: usize, rows: u8, columns: u8) -> (u8, u8) {
        let (rows, columns) = (usize::from(rows.max(1)), usize::from(columns.max(1)));
        let (row, column) = match self {
            Self::LeftToRight => (index / columns, index % columns),
            Self::RightToLeft => (index / columns, columns - 1 - index % columns),
            Self::TopToBottom => (index % rows, index / rows),
            Self::BottomToTop => (rows - 1 - index % rows, index / rows),
        };
        (u8::try_from(row).unwrap_or(u8::MAX), u8::try_from(column).unwrap_or(u8::MAX))
    }
}

/// What a slot holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// The udev `ID_PATH` of a whole disk: its `/dev/disk/by-path` name
    /// without `-partN`, e.g. `pci-0000:05:00.0-ata-1`.
    Port(String),
    /// A disk's serial number.
    Serial(String),
}

impl Binding {
    /// Whether `disk`, known by `serial_key` among the disks present
    /// ([`serial_key`]), is the one this binding names. A serial number
    /// two disks present report names neither: which one is meant cannot
    /// be told, and one must not take the other's slot.
    fn holds(&self, disk: &Disk, serial_key: Option<&str>) -> bool {
        match self {
            Binding::Port(port) => disk.id_path.as_deref().is_some_and(|path| same_port(port, path)),
            Binding::Serial(serial) => serial_key == Some(serial.as_str()),
        }
    }

    /// The form two bindings are compared in.
    fn key(&self) -> String {
        match self {
            Binding::Port(port) => format!("port {}", port_key(port)),
            Binding::Serial(serial) => format!("serial {serial}"),
        }
    }
}

/// Whether two port paths name the same port. udev names an ATA port both
/// `…-ata-1` and, since systemd 253, `…-ata-1.0` (with the link of a port
/// multiplier, 0 without one); both are the same port.
pub fn same_port(a: &str, b: &str) -> bool {
    port_key(a) == port_key(b)
}

fn port_key(path: &str) -> String {
    let mut key = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(at) = rest.find("-ata-") {
        let (before, after) = rest.split_at(at + "-ata-".len());
        key.push_str(before);
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        key.push_str(&after[..digits]);
        rest = &after[digits..];
        if digits > 0
            && let Some(tail) = rest.strip_prefix(".0")
            && (tail.is_empty() || tail.starts_with('-'))
        {
            rest = tail;
        }
    }
    key.push_str(rest);
    key
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub id: String,
    pub label: String,
    pub binding: Option<Binding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotGroup {
    pub id: String,
    pub name: String,
    pub rows: u8,
    pub columns: u8,
    pub orientation: Orientation,
    pub form: SlotForm,
    pub numbering: Numbering,
    /// In their numbering's order: one for each cell of the grid.
    pub slots: Vec<Slot>,
}

impl SlotGroup {
    /// Row and column of each slot, in the order of `slots`.
    pub fn cells(&self) -> impl Iterator<Item = (&Slot, (u8, u8))> {
        self.slots
            .iter()
            .enumerate()
            .map(|(index, slot)| (slot, self.numbering.cell(index, self.rows, self.columns)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidEnclosure {
    #[error("an enclosure has 1 to {MAX_GROUPS} groups of slots")]
    Groups,
    #[error("an id is 1 to {MAX_ID_CHARS} lowercase letters, digits or '-': {0:?}")]
    Id(String),
    #[error("the id {0:?} is used twice")]
    DuplicateId(String),
    #[error("a name is one line of 1 to {MAX_NAME_CHARS} characters")]
    Name,
    #[error("a slot's label is one line of 1 to {MAX_LABEL_CHARS} characters")]
    Label,
    #[error("a group has at least one row and column and at most {MAX_GROUP_SLOTS} slots")]
    Size,
    #[error("group {0:?} needs one slot for each place of its rows and columns")]
    SlotCount(String),
    #[error("not a port path (udev's ID_PATH of a whole disk): {0:?}")]
    Port(String),
    #[error("a serial number is one line of 1 to {MAX_SERIAL_CHARS} characters")]
    Serial,
    #[error("two slots are bound to the same disk or port: {0}")]
    DuplicateBinding(String),
    #[error("there is no slot {0:?}")]
    UnknownSlot(String),
    #[error("the slot {0:?} is bound otherwise twice")]
    DuplicateOverride(String),
    #[error("there is no model {0:?}")]
    UnknownModel(String),
}

/// A checked enclosure: 1 to [`MAX_GROUPS`] groups, every id unique, every
/// slot's label and binding valid and no disk or port bound twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    groups: Vec<SlotGroup>,
}

impl Layout {
    pub fn new(groups: Vec<SlotGroup>) -> Result<Self, InvalidEnclosure> {
        if groups.is_empty() || groups.len() > MAX_GROUPS {
            return Err(InvalidEnclosure::Groups);
        }
        let mut ids: Vec<&str> = Vec::new();
        let mut bound: Vec<String> = Vec::new();
        for group in &groups {
            check_id(&group.id)?;
            if ids.contains(&group.id.as_str()) {
                return Err(InvalidEnclosure::DuplicateId(group.id.clone()));
            }
            ids.push(&group.id);
            if !is_line(&group.name, MAX_NAME_CHARS) {
                return Err(InvalidEnclosure::Name);
            }
            let cells = usize::from(group.rows) * usize::from(group.columns);
            if cells == 0 || cells > MAX_GROUP_SLOTS {
                return Err(InvalidEnclosure::Size);
            }
            if group.slots.len() != cells {
                return Err(InvalidEnclosure::SlotCount(group.id.clone()));
            }
            for slot in &group.slots {
                check_id(&slot.id)?;
                if ids.contains(&slot.id.as_str()) {
                    return Err(InvalidEnclosure::DuplicateId(slot.id.clone()));
                }
                ids.push(&slot.id);
                if !is_line(&slot.label, MAX_LABEL_CHARS) {
                    return Err(InvalidEnclosure::Label);
                }
                if let Some(binding) = &slot.binding {
                    check_binding(binding)?;
                    let key = binding.key();
                    if bound.contains(&key) {
                        return Err(InvalidEnclosure::DuplicateBinding(key));
                    }
                    bound.push(key);
                }
            }
        }
        Ok(Self { groups })
    }

    pub fn groups(&self) -> &[SlotGroup] {
        &self.groups
    }

    pub fn slots(&self) -> impl Iterator<Item = &Slot> {
        self.groups.iter().flat_map(|group| &group.slots)
    }

    pub fn slot(&self, id: &str) -> Option<&Slot> {
        self.slots().find(|slot| slot.id == id)
    }

    /// The same layout with other bindings for some slots, each at most
    /// once.
    pub fn with_overrides(&self, overrides: &[Override]) -> Result<Self, InvalidEnclosure> {
        let mut groups = self.groups.clone();
        for (index, change) in overrides.iter().enumerate() {
            if overrides[..index].iter().any(|earlier| earlier.slot == change.slot) {
                return Err(InvalidEnclosure::DuplicateOverride(change.slot.clone()));
            }
            let slot = groups
                .iter_mut()
                .flat_map(|group| &mut group.slots)
                .find(|slot| slot.id == change.slot)
                .ok_or_else(|| InvalidEnclosure::UnknownSlot(change.slot.clone()))?;
            slot.binding.clone_from(&change.binding);
        }
        Self::new(groups)
    }
}

fn check_id(id: &str) -> Result<(), InvalidEnclosure> {
    let valid = (1..=MAX_ID_CHARS).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-');
    if valid { Ok(()) } else { Err(InvalidEnclosure::Id(id.to_owned())) }
}

/// One line, not blank, at most `max` characters, no spaces around it.
fn is_line(text: &str, max: usize) -> bool {
    let length = text.chars().count();
    length > 0 && length <= max && text.trim() == text && !text.chars().any(char::is_control)
}

fn check_binding(binding: &Binding) -> Result<(), InvalidEnclosure> {
    match binding {
        Binding::Port(port) => {
            let allowed = |b: u8| b.is_ascii_alphanumeric() || b":._+-".contains(&b);
            let partition = port
                .rsplit_once("-part")
                .is_some_and(|(_, number)| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()));
            if port.is_empty() || port.len() > MAX_PORT_CHARS || !port.bytes().all(allowed) || partition {
                return Err(InvalidEnclosure::Port(port.clone()));
            }
        }
        Binding::Serial(serial) => {
            if !is_line(serial, MAX_SERIAL_CHARS) {
                return Err(InvalidEnclosure::Serial);
            }
        }
    }
    Ok(())
}

/// Another binding for one slot of a model; `None` binds nothing there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub slot: String,
    pub binding: Option<Binding>,
}

/// What the firmware calls the machine: DMI's `sys_vendor` and
/// `product_name`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    pub vendor: String,
    pub product: String,
}

/// The machine a model is, by its DMI names, trimmed and whatever the case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmiMatch {
    pub vendor: String,
    pub product: String,
}

impl DmiMatch {
    pub fn matches(&self, machine: &Machine) -> bool {
        let same = |a: &str, b: &str| a.trim().to_lowercase() == b.trim().to_lowercase();
        !self.vendor.trim().is_empty()
            && same(&self.vendor, &machine.vendor)
            && same(&self.product, &machine.product)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChassisStyle {
    Tower,
    Rack,
    /// Just the slots, as for a layout of one's own.
    Plain,
}

impl ChassisStyle {
    pub const ALL: [ChassisStyle; 3] = [Self::Tower, Self::Rack, Self::Plain];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tower => "tower",
            Self::Rack => "rack",
            Self::Plain => "plain",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSide {
    Left,
    Right,
}

impl PanelSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Left, Self::Right].into_iter().find(|side| side.as_str() == value)
    }
}

/// A strip beside the bays with the power button and status lights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontPanel {
    pub side: PanelSide,
    pub power_button: bool,
    /// What each light shows, from the top: `power`, `network`, `disk1`, …
    pub leds: Vec<String>,
}

/// How the schematic draws the case around the slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chassis {
    pub style: ChassisStyle,
    pub panel: Option<FrontPanel>,
}

impl Chassis {
    pub const PLAIN: Chassis = Chassis { style: ChassisStyle::Plain, panel: None };
}

/// A NAS of the built-in catalog, with its slots bound to ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnclosureModel {
    pub id: String,
    pub vendor: String,
    pub name: String,
    pub dmi: DmiMatch,
    pub chassis: Chassis,
    pub layout: Layout,
    /// Someone checked the slots and ports on the real machine.
    pub verified: bool,
    pub notes: String,
}

/// The first model whose DMI names are the machine's.
pub fn detect<'a>(models: &'a [EnclosureModel], machine: &Machine) -> Option<&'a EnclosureModel> {
    models.iter().find(|model| model.dmi.matches(machine))
}

/// What the administrator chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnclosureChoice {
    /// A model of the catalog, with other bindings for some of its slots.
    Model { model: String, overrides: Vec<Override> },
    /// A layout of one's own.
    Custom(Layout),
}

impl EnclosureChoice {
    /// The layout to place disks in: the model's with the overrides, or
    /// the own one.
    pub fn layout(&self, models: &[EnclosureModel]) -> Result<Layout, InvalidEnclosure> {
        match self {
            EnclosureChoice::Model { model, overrides } => models
                .iter()
                .find(|m| &m.id == model)
                .ok_or_else(|| InvalidEnclosure::UnknownModel(model.clone()))?
                .layout
                .with_overrides(overrides),
            EnclosureChoice::Custom(layout) => Ok(layout.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// A present disk is in it.
    Occupied,
    /// No disk: the port bound has none, or nothing is bound.
    Empty,
    /// The disk bound by serial number is not there.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedSlot {
    pub slot: String,
    pub state: SlotState,
    /// The disk's kernel name, while occupied.
    pub disk: Option<String>,
}

/// Which disk is in which slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Every slot, in the layout's order.
    pub slots: Vec<PlacedSlot>,
    /// Present disks in no slot, by kernel name.
    pub unplaced: Vec<String>,
}

impl Placement {
    /// The slot a disk is in.
    pub fn slot_of(&self, disk: &str) -> Option<&str> {
        self.slots
            .iter()
            .find(|placed| placed.disk.as_deref() == Some(disk))
            .map(|placed| placed.slot.as_str())
    }
}

/// Places the present disks: by port first, then by serial number; a
/// disk is in one slot at most.
pub fn place(layout: &Layout, disks: &[Disk]) -> Placement {
    let keys: Vec<Option<String>> = disks.iter().map(|disk| serial_key(disk, disks)).collect();
    let mut taken = vec![false; disks.len()];
    let mut found: Vec<Option<usize>> = vec![None; layout.slots().count()];
    for port_pass in [true, false] {
        for (index, slot) in layout.slots().enumerate() {
            let Some(binding) = &slot.binding else { continue };
            if matches!(binding, Binding::Port(_)) != port_pass {
                continue;
            }
            let holds = |d: usize| binding.holds(&disks[d], keys[d].as_deref());
            if let Some(disk) = (0..disks.len()).find(|&d| !taken[d] && holds(d)) {
                taken[disk] = true;
                found[index] = Some(disk);
            }
        }
    }
    let slots = layout
        .slots()
        .zip(found)
        .map(|(slot, disk)| PlacedSlot {
            slot: slot.id.clone(),
            state: match (disk, &slot.binding) {
                (Some(_), _) => SlotState::Occupied,
                (None, Some(Binding::Serial(_))) => SlotState::Missing,
                (None, _) => SlotState::Empty,
            },
            disk: disk.map(|d| disks[d].name.clone()),
        })
        .collect();
    let unplaced =
        disks.iter().zip(taken).filter(|(_, taken)| !taken).map(|(disk, _)| disk.name.clone()).collect();
    Placement { slots, unplaced }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DiskUsage, Media, Transport};

    fn slot(id: &str, binding: Option<Binding>) -> Slot {
        Slot { id: id.to_owned(), label: id.to_uppercase(), binding }
    }

    fn port(path: &str) -> Option<Binding> {
        Some(Binding::Port(path.to_owned()))
    }

    fn serial(serial: &str) -> Option<Binding> {
        Some(Binding::Serial(serial.to_owned()))
    }

    fn group(id: &str, rows: u8, columns: u8, slots: Vec<Slot>) -> SlotGroup {
        SlotGroup {
            id: id.to_owned(),
            name: format!("Group {id}"),
            rows,
            columns,
            orientation: Orientation::Vertical,
            form: SlotForm::Inch35,
            numbering: Numbering::LeftToRight,
            slots,
        }
    }

    fn bays() -> SlotGroup {
        group(
            "bays",
            1,
            4,
            (1..=4).map(|n| slot(&format!("bay{n}"), port(&format!("pci-0000:05:00.0-ata-{n}")))).collect(),
        )
    }

    fn disk(name: &str, serial: Option<&str>, id_path: Option<&str>) -> Disk {
        Disk {
            name: name.to_owned(),
            model: None,
            serial: serial.map(str::to_owned),
            wwn: None,
            id_link: None,
            id_path: id_path.map(str::to_owned),
            size_bytes: 4 << 40,
            transport: Transport::Sata,
            media: Media::Rotational,
            removable: false,
            read_only: false,
            usage: DiskUsage::Unused,
            partitions: Vec::new(),
        }
    }

    #[test]
    fn values_read_as_they_are_written() {
        for orientation in Orientation::ALL {
            assert_eq!(Orientation::parse(orientation.as_str()), Some(orientation));
        }
        for form in SlotForm::ALL {
            assert_eq!(SlotForm::parse(form.as_str()), Some(form));
        }
        for numbering in Numbering::ALL {
            assert_eq!(Numbering::parse(numbering.as_str()), Some(numbering));
        }
        for style in ChassisStyle::ALL {
            assert_eq!(ChassisStyle::parse(style.as_str()), Some(style));
        }
        for side in [PanelSide::Left, PanelSide::Right] {
            assert_eq!(PanelSide::parse(side.as_str()), Some(side));
        }
        assert_eq!(SlotForm::parse("3.5"), None);
        assert_eq!(Orientation::parse("diagonal"), None);
        assert_eq!(Numbering::parse("spiral"), None);
        assert_eq!(ChassisStyle::parse("blade"), None);
        assert_eq!(PanelSide::parse("top"), None);
    }

    #[test]
    fn slots_fill_their_grid_in_the_numbering_s_order() {
        let cells = |numbering: Numbering| -> Vec<(u8, u8)> {
            (0..6).map(|index| numbering.cell(index, 2, 3)).collect()
        };
        assert_eq!(cells(Numbering::LeftToRight), [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)]);
        assert_eq!(cells(Numbering::RightToLeft), [(0, 2), (0, 1), (0, 0), (1, 2), (1, 1), (1, 0)]);
        assert_eq!(cells(Numbering::TopToBottom), [(0, 0), (1, 0), (0, 1), (1, 1), (0, 2), (1, 2)]);
        assert_eq!(cells(Numbering::BottomToTop), [(1, 0), (0, 0), (1, 1), (0, 1), (1, 2), (0, 2)]);
        let group = group("g", 2, 1, vec![slot("a", None), slot("b", None)]);
        let placed: Vec<_> = group.cells().map(|(slot, cell)| (slot.id.as_str(), cell)).collect();
        assert_eq!(placed, [("a", (0, 0)), ("b", (1, 0))]);
    }

    #[test]
    fn both_spellings_of_an_ata_port_are_the_same_port() {
        assert!(same_port("pci-0000:05:00.0-ata-1", "pci-0000:05:00.0-ata-1.0"));
        assert!(same_port("pci-0000:05:00.0-ata-1.0", "pci-0000:05:00.0-ata-1"));
        assert!(same_port("pci-0000:00:17.0-ata-12.0-sas-x", "pci-0000:00:17.0-ata-12-sas-x"));
        // Another link of a port multiplier is another disk.
        assert!(!same_port("pci-0000:05:00.0-ata-1.1", "pci-0000:05:00.0-ata-1"));
        assert!(!same_port("pci-0000:05:00.0-ata-1", "pci-0000:05:00.0-ata-10"));
        assert!(!same_port("pci-0000:05:00.0-ata-1.05", "pci-0000:05:00.0-ata-1"));
        // The PCI address's own `.0` stays.
        assert!(!same_port("pci-0000:05:00.0-nvme-1", "pci-0000:05:00-nvme-1"));
        assert!(same_port("pci-0000:01:00.0-nvme-1", "pci-0000:01:00.0-nvme-1"));
        assert!(!same_port("platform-ata-", "platform-ata-.0"));
    }

    #[test]
    fn a_layout_is_checked_whole() {
        let layout = Layout::new(vec![bays()]).unwrap();
        assert_eq!(layout.groups().len(), 1);
        assert_eq!(layout.slots().count(), 4);
        assert_eq!(layout.slot("bay3").unwrap().label, "BAY3");
        assert!(layout.slot("bay5").is_none());

        let invalid = |groups: Vec<SlotGroup>| Layout::new(groups).unwrap_err();
        assert_eq!(invalid(vec![]), InvalidEnclosure::Groups);
        let many: Vec<SlotGroup> = (0..=MAX_GROUPS)
            .map(|n| group(&format!("g{n}"), 1, 1, vec![slot(&format!("s{n}"), None)]))
            .collect();
        assert_eq!(invalid(many), InvalidEnclosure::Groups);
        for id in ["", "Bays", "bay 1", "-bay", &"x".repeat(MAX_ID_CHARS + 1)] {
            let mut bays = bays();
            bays.id = id.to_owned();
            assert_eq!(invalid(vec![bays]), InvalidEnclosure::Id(id.to_owned()), "{id:?}");
        }
        let mut twice = bays();
        twice.slots[1].id = "bay1".into();
        assert_eq!(invalid(vec![twice]), InvalidEnclosure::DuplicateId("bay1".into()));
        let mut same_as_group = bays();
        same_as_group.slots[0].id = "bays".into();
        assert_eq!(invalid(vec![same_as_group]), InvalidEnclosure::DuplicateId("bays".into()));
        let second = group("bays", 1, 1, vec![slot("x", None)]);
        assert_eq!(invalid(vec![bays(), second]), InvalidEnclosure::DuplicateId("bays".into()));
        for name in ["", " Front", "Front\nback", &"n".repeat(MAX_NAME_CHARS + 1)] {
            let mut bays = bays();
            bays.name = name.to_owned();
            assert_eq!(invalid(vec![bays]), InvalidEnclosure::Name, "{name:?}");
        }
        let mut label = bays();
        label.slots[2].label = "x".repeat(MAX_LABEL_CHARS + 1);
        assert_eq!(invalid(vec![label]), InvalidEnclosure::Label);
        let mut unlabelled = bays();
        unlabelled.slots[2].label = "  ".into();
        assert_eq!(invalid(vec![unlabelled]), InvalidEnclosure::Label);
        assert_eq!(invalid(vec![group("g", 0, 4, vec![])]), InvalidEnclosure::Size);
        let big: Vec<Slot> = (0..61).map(|n| slot(&format!("s{n}"), None)).collect();
        assert_eq!(invalid(vec![group("g", 61, 1, big)]), InvalidEnclosure::Size);
        let mut short = bays();
        short.slots.pop();
        assert_eq!(invalid(vec![short]), InvalidEnclosure::SlotCount("bays".into()));
        let full: Vec<Slot> = (0..60).map(|n| slot(&format!("s{n}"), None)).collect();
        assert!(Layout::new(vec![group("g", 6, 10, full)]).is_ok());
    }

    #[test]
    fn bindings_are_checked_and_never_bound_twice() {
        let with = |binding: Option<Binding>| {
            let mut bays = bays();
            bays.slots[0].binding = binding;
            Layout::new(vec![bays])
        };
        assert!(with(port("pci-0000:00:0d.0-usb-0:1:1.0-scsi-0:0:0:0")).is_ok());
        assert!(with(port("platform-ahci_0+x")).is_ok());
        assert!(with(serial("WD-WX12 345")).is_ok());
        assert!(with(None).is_ok());
        for bad in ["", "pci-0000:05:00.0-ata-1-part1", "../sda", "pci 1", &"p".repeat(129)] {
            assert_eq!(with(port(bad)).unwrap_err(), InvalidEnclosure::Port(bad.to_owned()), "{bad:?}");
        }
        assert!(with(port("pci-0000:05:00.0-ata-1-partition")).is_ok(), "not a partition's number");
        assert!(with(port("pci-0000:05:00.0-ata-9-part")).is_ok(), "no number: not a partition");
        for bad in ["", " A", "A\tB", &"s".repeat(65)] {
            assert_eq!(with(serial(bad)).unwrap_err(), InvalidEnclosure::Serial, "{bad:?}");
        }
        // bay2 is bound to ata-2 already, in the other spelling.
        assert_eq!(
            with(port("pci-0000:05:00.0-ata-2.0")).unwrap_err(),
            InvalidEnclosure::DuplicateBinding("port pci-0000:05:00.0-ata-2".into())
        );
        let mut two = bays();
        two.slots[0].binding = serial("S1");
        two.slots[1].binding = serial("S1");
        assert_eq!(
            Layout::new(vec![two]).unwrap_err(),
            InvalidEnclosure::DuplicateBinding("serial S1".into())
        );
    }

    #[test]
    fn overrides_bind_a_model_s_slots_otherwise() {
        let layout = Layout::new(vec![bays()]).unwrap();
        let changed = layout
            .with_overrides(&[
                Override { slot: "bay1".into(), binding: None },
                Override { slot: "bay2".into(), binding: serial("S2") },
            ])
            .unwrap();
        assert_eq!(changed.slot("bay1").unwrap().binding, None);
        assert_eq!(changed.slot("bay2").unwrap().binding, serial("S2"));
        assert_eq!(changed.slot("bay3").unwrap().binding, port("pci-0000:05:00.0-ata-3"));
        let unknown = layout.with_overrides(&[Override { slot: "bay9".into(), binding: None }]);
        assert_eq!(unknown.unwrap_err(), InvalidEnclosure::UnknownSlot("bay9".into()));
        let twice = layout
            .with_overrides(&[Override { slot: "bay1".into(), binding: port("pci-0000:05:00.0-ata-4") }]);
        assert!(matches!(twice, Err(InvalidEnclosure::DuplicateBinding(_))));
    }

    #[test]
    fn a_slot_is_bound_otherwise_once_at_most() {
        let layout = Layout::new(vec![bays()]).unwrap();
        let again = layout.with_overrides(&[
            Override { slot: "bay1".into(), binding: None },
            Override { slot: "bay2".into(), binding: serial("S2") },
            Override { slot: "bay1".into(), binding: serial("S1") },
        ]);
        assert_eq!(again.unwrap_err(), InvalidEnclosure::DuplicateOverride("bay1".into()));
    }

    fn model() -> EnclosureModel {
        EnclosureModel {
            id: "acme-4".into(),
            vendor: "ACME".into(),
            name: "Four".into(),
            dmi: DmiMatch { vendor: "ACME ".into(), product: "four plus".into() },
            chassis: Chassis::PLAIN,
            layout: Layout::new(vec![bays()]).unwrap(),
            verified: false,
            notes: String::new(),
        }
    }

    #[test]
    fn models_are_detected_by_their_dmi_names() {
        let models = [model()];
        let machine =
            |vendor: &str, product: &str| Machine { vendor: vendor.into(), product: product.into() };
        assert_eq!(detect(&models, &machine("acme", " Four Plus\n")).map(|m| m.id.as_str()), Some("acme-4"));
        assert!(detect(&models, &machine("ACME", "Four")).is_none());
        assert!(detect(&models, &machine("QEMU", "Standard PC (Q35 + ICH9, 2009)")).is_none());
        assert!(detect(&models, &Machine::default()).is_none());
        let blank = DmiMatch { vendor: " ".into(), product: String::new() };
        assert!(!blank.matches(&Machine::default()), "an empty DMI matches no model");
    }

    #[test]
    fn a_choice_gives_the_layout_to_place_disks_in() {
        let models = [model()];
        let chosen = EnclosureChoice::Model {
            model: "acme-4".into(),
            overrides: vec![Override { slot: "bay4".into(), binding: None }],
        };
        assert_eq!(chosen.layout(&models).unwrap().slot("bay4").unwrap().binding, None);
        let gone = EnclosureChoice::Model { model: "acme-8".into(), overrides: vec![] };
        assert_eq!(gone.layout(&models).unwrap_err(), InvalidEnclosure::UnknownModel("acme-8".into()));
        let own = Layout::new(vec![group("g", 1, 1, vec![slot("a", None)])]).unwrap();
        assert_eq!(EnclosureChoice::Custom(own.clone()).layout(&models).unwrap(), own);
    }

    #[test]
    fn disks_are_placed_by_port_first_then_by_serial() {
        let mut bays = bays();
        bays.slots[3].binding = serial("S9");
        let usb = group("usb", 1, 2, vec![slot("usb1", serial("S5")), slot("usb2", serial("S7"))]);
        let layout = Layout::new(vec![bays, usb]).unwrap();
        let disks = [
            disk("sda", Some("S1"), Some("pci-0000:05:00.0-ata-1.0")),
            // In bay 3's port; its serial is also bound to usb2, but the
            // port comes first.
            disk("sdc", Some("S7"), Some("pci-0000:05:00.0-ata-3")),
            disk("sde", Some("S5"), Some("pci-0000:00:0d.0-usb-0:1:1.0-scsi-0:0:0:0")),
            disk("sdf", None, None),
            disk("nvme0n1", Some("N1"), Some("pci-0000:04:00.0-nvme-1")),
        ];
        let placement = place(&layout, &disks);
        let states: Vec<_> =
            placement.slots.iter().map(|p| (p.slot.as_str(), p.state, p.disk.as_deref())).collect();
        assert_eq!(
            states,
            [
                ("bay1", SlotState::Occupied, Some("sda")),
                ("bay2", SlotState::Empty, None),
                ("bay3", SlotState::Occupied, Some("sdc")),
                ("bay4", SlotState::Missing, None),
                ("usb1", SlotState::Occupied, Some("sde")),
                ("usb2", SlotState::Missing, None),
            ]
        );
        assert_eq!(placement.unplaced, ["sdf", "nvme0n1"]);
        assert_eq!(placement.slot_of("sde"), Some("usb1"));
        assert_eq!(placement.slot_of("sdf"), None);

        let unbound = Layout::new(vec![group("g", 1, 1, vec![slot("a", None)])]).unwrap();
        let placement = place(&unbound, &disks[..1]);
        assert_eq!(placement.slots[0].state, SlotState::Empty);
        assert_eq!(placement.unplaced, ["sda"]);
    }

    #[test]
    fn a_serial_number_two_disks_report_places_neither() {
        // A two-bay USB dock whose bridge reports its own serial number for
        // both disks; the slots are bound to it, and to their ports.
        let dock = "pci-0000:00:14.0-usb-0:2:1.0-scsi-0:0:0";
        let usb = group(
            "usb",
            1,
            3,
            vec![
                slot("usb1", serial("BRIDGE1")),
                slot("usb2", port(&format!("{dock}:1"))),
                slot("usb3", serial("WD-1")),
            ],
        );
        let layout = Layout::new(vec![usb]).unwrap();
        let first = disk("sdf", Some("BRIDGE1"), Some(&format!("{dock}:0")));
        let second = disk("sdg", Some("BRIDGE1"), Some(&format!("{dock}:1")));
        let own = disk("sdh", Some("WD-1"), None);

        let placement = place(&layout, &[first.clone(), second.clone(), own.clone()]);
        let states: Vec<_> =
            placement.slots.iter().map(|p| (p.slot.as_str(), p.state, p.disk.as_deref())).collect();
        assert_eq!(
            states,
            [
                ("usb1", SlotState::Missing, None),
                ("usb2", SlotState::Occupied, Some("sdg")),
                ("usb3", SlotState::Occupied, Some("sdh")),
            ]
        );
        assert_eq!(placement.unplaced, ["sdf"], "which of the two it was cannot be told");

        // Alone in the dock, its serial number is its own again.
        let placement = place(&layout, &[first, own]);
        assert_eq!(placement.slot_of("sdf"), Some("usb1"));
    }
}
