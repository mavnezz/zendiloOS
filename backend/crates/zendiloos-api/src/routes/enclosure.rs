//! Where each disk sits: the enclosure, making a disk blink, and the
//! inventory of every disk seen.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::domain::Permission;
use zendiloos_storage::application::enclosure::{EnclosureView, LocateSession};
use zendiloos_storage::application::inventory::InventoryEntry;
use zendiloos_storage::domain as storage;

use super::storage::DiskHealth;
use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, ApiPath as Path, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(enclosure, set_enclosure, reset_enclosure))
        .routes(routes!(models))
        .routes(routes!(locating))
        .routes(routes!(locate, stop_locate))
        .routes(routes!(inventory))
        .routes(routes!(update_inventory, remove_inventory))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// Side by side and upright, as at the front of a tower NAS.
    Vertical,
    /// On top of each other and lying, as in a rack or a cage of 2.5" bays.
    Horizontal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotForm {
    /// 3.5" drives.
    #[serde(rename = "inch_3_5")]
    Inch35,
    /// 2.5" drives.
    #[serde(rename = "inch_2_5")]
    Inch25,
    M2,
}

/// In which order a group's slots fill its grid: along the rows from the
/// top row, or down the columns from the left one.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Numbering {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    /// Where a disk is plugged in: udev's `ID_PATH` of a whole disk, its
    /// `/dev/disk/by-path` name without `-partN`. A disk replaced in the
    /// slot shows up there by itself.
    Port,
    /// A disk's serial number, for ports that are not stable (USB, some
    /// HBAs).
    Serial,
}

/// What a slot holds.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SlotBinding {
    pub kind: BindingKind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChassisStyle {
    Tower,
    Rack,
    /// Just the slots.
    Plain,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PanelSide {
    Left,
    Right,
}

/// A strip beside the bays with the power button and status lights.
#[derive(Debug, Serialize, ToSchema)]
pub struct FrontPanel {
    pub side: PanelSide,
    pub power_button: bool,
    /// What each light shows, from the top: `power`, `network`, `disk1`, …
    pub leds: Vec<String>,
}

/// How the schematic draws the case around the slots.
#[derive(Debug, Serialize, ToSchema)]
pub struct Chassis {
    pub style: ChassisStyle,
    pub panel: Option<FrontPanel>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelSlot {
    pub id: String,
    pub label: String,
    /// From 0 at the top of the group.
    pub row: u8,
    /// From 0 at the left of the group.
    pub column: u8,
    pub binding: Option<SlotBinding>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelGroup {
    pub id: String,
    pub name: String,
    pub rows: u8,
    pub columns: u8,
    pub orientation: Orientation,
    pub form: SlotForm,
    pub numbering: Numbering,
    /// In their numbering's order.
    pub slots: Vec<ModelSlot>,
}

/// A NAS of the built-in catalog.
#[derive(Debug, Serialize, ToSchema)]
pub struct EnclosureModel {
    pub id: String,
    pub vendor: String,
    pub name: String,
    /// DMI's `sys_vendor` and `product_name` of the machine.
    pub dmi_vendor: String,
    pub dmi_product: String,
    /// Someone checked the slots and ports on the real machine.
    pub verified: bool,
    pub notes: String,
    pub chassis: Chassis,
    pub groups: Vec<ModelGroup>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnclosureSource {
    /// A model of the catalog.
    Model,
    /// A layout of one's own.
    Custom,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotState {
    /// A present disk is in it.
    Occupied,
    /// No disk: the port bound has none, or nothing is bound.
    Empty,
    /// The disk bound by serial number is not there.
    Missing,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PlacedSlot {
    pub id: String,
    pub label: String,
    pub row: u8,
    pub column: u8,
    pub binding: Option<SlotBinding>,
    /// Bound otherwise than the model binds it.
    pub overridden: bool,
    pub state: SlotState,
    /// The disk in it, by kernel name.
    pub disk: Option<String>,
    /// The disk's SMART verdict as the monitor last read it (every six
    /// hours, disks holding data only); reading this reads no disk.
    pub health: Option<DiskHealth>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PlacedGroup {
    pub id: String,
    pub name: String,
    pub rows: u8,
    pub columns: u8,
    pub orientation: Orientation,
    pub form: SlotForm,
    pub numbering: Numbering,
    pub slots: Vec<PlacedSlot>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Machine {
    pub vendor: String,
    pub product: String,
}

/// The machine's enclosure and which disk is in which slot.
#[derive(Debug, Serialize, ToSchema)]
pub struct Enclosure {
    /// What the firmware calls the machine (DMI); empty where it says
    /// nothing.
    pub machine: Machine,
    /// The catalog's model for this machine, offered while nothing is
    /// chosen.
    pub detected: Option<EnclosureModel>,
    /// What was chosen; `null`: not set up yet.
    pub source: Option<EnclosureSource>,
    /// The model chosen, with its own bindings, for `model`.
    pub model: Option<EnclosureModel>,
    /// The model's, or plain for a layout of one's own.
    pub chassis: Chassis,
    /// The groups disks are placed in, the model's overrides applied;
    /// empty while not set up.
    pub groups: Vec<PlacedGroup>,
    /// Present disks in no slot, by kernel name.
    pub unplaced: Vec<String>,
}

fn binding_dto(binding: Option<&storage::Binding>) -> Option<SlotBinding> {
    binding.map(|binding| match binding {
        storage::Binding::Port(port) => SlotBinding { kind: BindingKind::Port, value: port.clone() },
        storage::Binding::Serial(serial) => SlotBinding { kind: BindingKind::Serial, value: serial.clone() },
    })
}

fn orientation(value: storage::Orientation) -> Orientation {
    match value {
        storage::Orientation::Vertical => Orientation::Vertical,
        storage::Orientation::Horizontal => Orientation::Horizontal,
    }
}

fn form(value: storage::SlotForm) -> SlotForm {
    match value {
        storage::SlotForm::Inch35 => SlotForm::Inch35,
        storage::SlotForm::Inch25 => SlotForm::Inch25,
        storage::SlotForm::M2 => SlotForm::M2,
    }
}

fn numbering(value: storage::Numbering) -> Numbering {
    match value {
        storage::Numbering::LeftToRight => Numbering::LeftToRight,
        storage::Numbering::RightToLeft => Numbering::RightToLeft,
        storage::Numbering::TopToBottom => Numbering::TopToBottom,
        storage::Numbering::BottomToTop => Numbering::BottomToTop,
    }
}

fn chassis(chassis: &storage::Chassis) -> Chassis {
    Chassis {
        style: match chassis.style {
            storage::ChassisStyle::Tower => ChassisStyle::Tower,
            storage::ChassisStyle::Rack => ChassisStyle::Rack,
            storage::ChassisStyle::Plain => ChassisStyle::Plain,
        },
        panel: chassis.panel.as_ref().map(|panel| FrontPanel {
            side: match panel.side {
                storage::PanelSide::Left => PanelSide::Left,
                storage::PanelSide::Right => PanelSide::Right,
            },
            power_button: panel.power_button,
            leds: panel.leds.clone(),
        }),
    }
}

impl From<&storage::EnclosureModel> for EnclosureModel {
    fn from(model: &storage::EnclosureModel) -> Self {
        Self {
            id: model.id.clone(),
            vendor: model.vendor.clone(),
            name: model.name.clone(),
            dmi_vendor: model.dmi.vendor.clone(),
            dmi_product: model.dmi.product.clone(),
            verified: model.verified,
            notes: model.notes.clone(),
            chassis: chassis(&model.chassis),
            groups: model
                .layout
                .groups()
                .iter()
                .map(|group| ModelGroup {
                    id: group.id.clone(),
                    name: group.name.clone(),
                    rows: group.rows,
                    columns: group.columns,
                    orientation: orientation(group.orientation),
                    form: form(group.form),
                    numbering: numbering(group.numbering),
                    slots: group
                        .cells()
                        .map(|(slot, (row, column))| ModelSlot {
                            id: slot.id.clone(),
                            label: slot.label.clone(),
                            row,
                            column,
                            binding: binding_dto(slot.binding.as_ref()),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

impl From<EnclosureView> for Enclosure {
    fn from(view: EnclosureView) -> Self {
        let machine = Machine { vendor: view.machine.vendor, product: view.machine.product };
        let detected = view.detected.as_ref().map(EnclosureModel::from);
        let Some(chosen) = view.chosen else {
            return Self {
                machine,
                detected,
                source: None,
                model: None,
                chassis: chassis(&storage::Chassis::PLAIN),
                groups: Vec::new(),
                unplaced: Vec::new(),
            };
        };
        let groups = chosen
            .layout
            .groups()
            .iter()
            .map(|group| PlacedGroup {
                id: group.id.clone(),
                name: group.name.clone(),
                rows: group.rows,
                columns: group.columns,
                orientation: orientation(group.orientation),
                form: form(group.form),
                numbering: numbering(group.numbering),
                slots: group
                    .cells()
                    .map(|(slot, (row, column))| {
                        let placed = chosen.placement.slots.iter().find(|p| p.slot == slot.id);
                        let disk = placed.and_then(|p| p.disk.clone());
                        PlacedSlot {
                            id: slot.id.clone(),
                            label: slot.label.clone(),
                            row,
                            column,
                            binding: binding_dto(slot.binding.as_ref()),
                            overridden: chosen.overridden.contains(&slot.id),
                            state: match placed.map_or(storage::SlotState::Empty, |p| p.state) {
                                storage::SlotState::Occupied => SlotState::Occupied,
                                storage::SlotState::Empty => SlotState::Empty,
                                storage::SlotState::Missing => SlotState::Missing,
                            },
                            health: disk.as_ref().and_then(|d| chosen.health.get(d)).map(|h| (*h).into()),
                            disk,
                        }
                    })
                    .collect(),
            })
            .collect();
        Self {
            machine,
            detected,
            source: Some(if chosen.model.is_some() {
                EnclosureSource::Model
            } else {
                EnclosureSource::Custom
            }),
            chassis: chosen.model.as_ref().map_or(chassis(&storage::Chassis::PLAIN), |m| chassis(&m.chassis)),
            model: chosen.model.as_ref().map(EnclosureModel::from),
            groups,
            unplaced: chosen.placement.unplaced,
        }
    }
}

/// The machine's enclosure: the model or own layout chosen, which disk is
/// in which slot, the disks in none; while nothing is chosen, the model
/// the machine was recognised as. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/enclosure",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Enclosure),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn enclosure(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Enclosure>, ApiError> {
    auth.require(Permission::StorageView)?;
    Ok(Json(state.storage.enclosure.view(&client).await?.into()))
}

/// Another binding for one of the model's slots; `null` binds nothing.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SlotOverride {
    pub slot: String,
    pub binding: Option<SlotBinding>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewSlot {
    /// 1 to 32 lowercase letters, digits or `-`, unique in the enclosure.
    pub id: String,
    /// 1 to 32 characters.
    pub label: String,
    #[serde(default)]
    pub binding: Option<SlotBinding>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewGroup {
    pub id: String,
    /// 1 to 64 characters.
    pub name: String,
    /// Rows × columns: 1 to 60.
    pub rows: u8,
    pub columns: u8,
    pub orientation: Orientation,
    pub form: SlotForm,
    pub numbering: Numbering,
    /// One for each place of the grid, in the numbering's order.
    pub slots: Vec<NewSlot>,
}

/// A model of the catalog with other bindings for some of its slots, or a
/// layout of one's own (1 to 16 groups).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum SetEnclosure {
    Model {
        model: String,
        #[serde(default)]
        overrides: Vec<SlotOverride>,
    },
    Custom {
        groups: Vec<NewGroup>,
    },
}

fn binding(binding: Option<SlotBinding>) -> Option<storage::Binding> {
    binding.map(|binding| match binding.kind {
        BindingKind::Port => storage::Binding::Port(binding.value),
        BindingKind::Serial => storage::Binding::Serial(binding.value),
    })
}

impl TryFrom<SetEnclosure> for storage::EnclosureChoice {
    type Error = storage::InvalidEnclosure;

    fn try_from(request: SetEnclosure) -> Result<Self, Self::Error> {
        Ok(match request {
            SetEnclosure::Model { model, overrides } => storage::EnclosureChoice::Model {
                model,
                overrides: overrides
                    .into_iter()
                    .map(|o| storage::Override { slot: o.slot, binding: binding(o.binding) })
                    .collect(),
            },
            SetEnclosure::Custom { groups } => storage::EnclosureChoice::Custom(storage::Layout::new(
                groups
                    .into_iter()
                    .map(|group| storage::SlotGroup {
                        id: group.id,
                        name: group.name,
                        rows: group.rows,
                        columns: group.columns,
                        orientation: match group.orientation {
                            Orientation::Vertical => storage::Orientation::Vertical,
                            Orientation::Horizontal => storage::Orientation::Horizontal,
                        },
                        form: match group.form {
                            SlotForm::Inch35 => storage::SlotForm::Inch35,
                            SlotForm::Inch25 => storage::SlotForm::Inch25,
                            SlotForm::M2 => storage::SlotForm::M2,
                        },
                        numbering: match group.numbering {
                            Numbering::LeftToRight => storage::Numbering::LeftToRight,
                            Numbering::RightToLeft => storage::Numbering::RightToLeft,
                            Numbering::TopToBottom => storage::Numbering::TopToBottom,
                            Numbering::BottomToTop => storage::Numbering::BottomToTop,
                        },
                        slots: group
                            .slots
                            .into_iter()
                            .map(|slot| storage::Slot {
                                id: slot.id,
                                label: slot.label,
                                binding: binding(slot.binding),
                            })
                            .collect(),
                    })
                    .collect(),
            )?),
        })
    }
}

/// Chooses the enclosure: a model of the catalog (with other bindings for
/// some slots, each slot once at most), or a layout of one's own. Saved,
/// it is answered also while the disks cannot be read, with none placed.
/// Needs `storage.manage`.
#[utoipa::path(
    put,
    path = "/storage/enclosure",
    tag = "storage",
    security(("session" = [])),
    request_body = SetEnclosure,
    responses(
        (status = OK, body = Enclosure),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`, also for a slot bound otherwise twice"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn set_enclosure(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<SetEnclosure>,
) -> Result<Json<Enclosure>, ApiError> {
    auth.require(Permission::StorageManage)?;
    let choice =
        storage::EnclosureChoice::try_from(request).map_err(|e| ApiError::InvalidInput(e.to_string()))?;
    Ok(Json(state.storage.enclosure.choose(Some(choice), &auth.0.actor(), &client).await?.into()))
}

/// Sets no enclosure up: the disks are listed without slots. Needs
/// `storage.manage`.
#[utoipa::path(
    delete,
    path = "/storage/enclosure",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Enclosure),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn reset_enclosure(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Enclosure>, ApiError> {
    auth.require(Permission::StorageManage)?;
    Ok(Json(state.storage.enclosure.choose(None, &auth.0.actor(), &client).await?.into()))
}

/// The NAS models zendiloOS knows. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/enclosure/models",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<EnclosureModel>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn models(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<EnclosureModel>>, ApiError> {
    auth.require(Permission::StorageView)?;
    Ok(Json(state.storage.enclosure.models().iter().map(EnclosureModel::from).collect()))
}

/// A disk blinking: read from in bursts, so its activity light flashes.
#[derive(Debug, Serialize, ToSchema)]
pub struct Locating {
    pub disk: String,
    /// When it stops, Unix seconds.
    pub until: i64,
}

impl From<LocateSession> for Locating {
    fn from(session: LocateSession) -> Self {
        Self { disk: session.disk, until: session.until.unix_seconds() }
    }
}

/// The disks blinking now. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/locate",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<Locating>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn locating(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<Locating>>, ApiError> {
    auth.require(Permission::StorageView)?;
    Ok(Json(state.storage.locate.sessions().into_iter().map(Locating::from).collect()))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LocateRequest {
    /// How long it blinks: 1 to 300 seconds, 60 if not given.
    #[serde(default)]
    pub seconds: Option<u16>,
}

/// Has a disk blink so it can be found in its slot: it is read from (never
/// written) in bursts of 0.7 seconds with pauses between, which flashes
/// its activity light; a disk asleep wakes. Asked again, it starts over.
/// NVMe slots mostly have no light. Needs `storage.manage`.
#[utoipa::path(
    post,
    path = "/storage/disks/{name}/locate",
    tag = "storage",
    security(("session" = [])),
    params(("name" = String, Path)),
    request_body = LocateRequest,
    responses(
        (status = OK, body = Locating),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn locate(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
    ApiJson(request): ApiJson<LocateRequest>,
) -> Result<Json<Locating>, ApiError> {
    auth.require(Permission::StorageManage)?;
    let session = state.storage.locate.start(&name, request.seconds, &auth.0.actor(), &client).await?;
    Ok(Json(session.into()))
}

/// Stops a disk blinking. Needs `storage.manage`.
#[utoipa::path(
    delete,
    path = "/storage/disks/{name}/locate",
    tag = "storage",
    security(("session" = [])),
    params(("name" = String, Path)),
    responses(
        (status = NO_CONTENT, description = "Stopped."),
        (status = CONFLICT, body = Problem, description = "`not_locating`"),
    )
)]
async fn stop_locate(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::StorageManage)?;
    state.storage.locate.stop(&name, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A disk the NAS has seen, by serial number.
#[derive(Debug, Serialize, ToSchema)]
pub struct InventoryDisk {
    /// Its serial number; while another disk present reports the same one
    /// (the disks of some USB docks), with its port: `SERIAL@ID_PATH`.
    pub serial: String,
    /// As last seen.
    pub model: Option<String>,
    pub size_bytes: u64,
    /// `YYYY-MM-DD`.
    pub purchased: Option<String>,
    /// `YYYY-MM-DD`.
    pub warranty_until: Option<String>,
    /// Where it was bought.
    pub vendor: Option<String>,
    pub note: Option<String>,
    /// Unix seconds.
    pub first_seen: i64,
    /// Unix seconds; while present, up to an hour behind. Never goes back.
    pub last_seen: i64,
    /// Its kernel name while present; `null`: stored, no longer present
    /// (or, in the answer to a change, not known while the disks cannot be
    /// read).
    pub disk: Option<String>,
}

impl From<InventoryEntry> for InventoryDisk {
    fn from(entry: InventoryEntry) -> Self {
        let record = entry.record;
        Self {
            serial: record.serial,
            model: record.model,
            size_bytes: record.size_bytes,
            purchased: record.details.purchased.map(|date| date.to_string()),
            warranty_until: record.details.warranty_until.map(|date| date.to_string()),
            vendor: record.details.vendor,
            note: record.details.note,
            first_seen: record.first_seen.unix_seconds(),
            last_seen: record.last_seen.unix_seconds(),
            disk: entry.disk,
        }
    }
}

/// Every disk seen: those present first, then the others, most recently
/// seen first. Needs `storage.view`.
#[utoipa::path(
    get,
    path = "/storage/inventory",
    tag = "storage",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<InventoryDisk>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn inventory(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
) -> Result<Json<Vec<InventoryDisk>>, ApiError> {
    auth.require(Permission::StorageView)?;
    Ok(Json(state.storage.inventory.list(&client).await?.into_iter().map(InventoryDisk::from).collect()))
}

/// What the owner notes about a disk; blank or absent is none.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InventoryUpdate {
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub purchased: Option<String>,
    /// `YYYY-MM-DD`, not before `purchased`.
    #[serde(default)]
    pub warranty_until: Option<String>,
    /// At most 64 characters.
    #[serde(default)]
    pub vendor: Option<String>,
    /// At most 500 characters.
    #[serde(default)]
    pub note: Option<String>,
}

/// Notes when and where a disk was bought and until when its warranty
/// holds. Needs `storage.manage`.
#[utoipa::path(
    put,
    path = "/storage/inventory/{serial}",
    tag = "storage",
    security(("session" = [])),
    params(("serial" = String, Path)),
    request_body = InventoryUpdate,
    responses(
        (status = OK, body = InventoryDisk),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn update_inventory(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(serial): Path<String>,
    ApiJson(request): ApiJson<InventoryUpdate>,
) -> Result<Json<InventoryDisk>, ApiError> {
    auth.require(Permission::StorageManage)?;
    let details = storage::InventoryDetails::new(
        request.purchased.as_deref(),
        request.warranty_until.as_deref(),
        request.vendor.as_deref(),
        request.note.as_deref(),
    )
    .map_err(|e| ApiError::InvalidInput(e.to_string()))?;
    let entry = state.storage.inventory.update(&serial, details, &auth.0.actor(), &client).await?;
    Ok(Json(entry.into()))
}

/// Forgets a disk that is no longer present. Needs `storage.manage`.
#[utoipa::path(
    delete,
    path = "/storage/inventory/{serial}",
    tag = "storage",
    security(("session" = [])),
    params(("serial" = String, Path)),
    responses(
        (status = NO_CONTENT, description = "Forgotten."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`disk_present`"),
    )
)]
async fn remove_inventory(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(serial): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::StorageManage)?;
    state.storage.inventory.remove(&serial, &auth.0.actor(), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
