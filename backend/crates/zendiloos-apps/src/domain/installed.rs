//! Apps installed from a catalog: the version installed, the values
//! zendiloOS filled in for it, and what Docker makes of it.

use std::collections::{BTreeMap, BTreeSet};

use zendiloos_kernel::Timestamp;

use super::{CatalogApp, Setting, SettingKind, WebScheme};

/// The longest app id zendiloOS installs: the app's account, `app-<id>`,
/// has at most 32 characters.
pub const MAX_APP_ID: usize = 28;

/// Host ports zendiloOS itself or the NAS's file services use; the
/// catalog's format refuses them as defaults too.
pub const RESERVED_PORTS: [u16; 11] = [22, 80, 111, 137, 138, 139, 443, 445, 2049, 3702, 5353];

/// Longest value of a setting, in bytes.
pub const MAX_SETTING_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    /// Being installed or updated: a job runs.
    Installing,
    Installed,
    /// The last installation or update did not finish: it can be tried
    /// again, or the app removed.
    Failed,
    /// Being removed: a job runs.
    Removing,
}

impl InstallState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installing => "installing",
            Self::Installed => "installed",
            Self::Failed => "failed",
            Self::Removing => "removing",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "installing" => Some(Self::Installing),
            "installed" => Some(Self::Installed),
            "failed" => Some(Self::Failed),
            "removing" => Some(Self::Removing),
            _ => None,
        }
    }

    /// A job is working on the app.
    pub fn is_busy(self) -> bool {
        matches!(self, Self::Installing | Self::Removing)
    }
}

/// What zendiloOS filled in for an app, by the ids its manifest declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallValues {
    /// Share id → the share picked for it: its name, or a folder in it
    /// (`media/Films`), see [`share_and_folder`].
    pub shares: BTreeMap<String, String>,
    /// Port id → host port.
    pub ports: BTreeMap<String, u16>,
    /// Setting id → value. Generated ones are kept across updates.
    pub settings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    /// The catalog's id: also the Compose project.
    pub id: String,
    /// The catalog it came from; it may have been removed since.
    pub source_id: i64,
    /// The version installed: manifest, compose file and digest.
    pub app: CatalogApp,
    /// Where its data is: the pool Docker's data is on.
    pub pool: String,
    pub volume: u16,
    pub values: InstallValues,
    pub state: InstallState,
    pub installed_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Where an app's web interface is, on the NAS's own address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebAddress {
    pub scheme: WebScheme,
    pub port: u16,
    pub path: String,
}

impl InstalledApp {
    /// The Linux account the app runs as.
    pub fn account(&self) -> String {
        account(&self.id)
    }

    pub fn web(&self) -> Option<WebAddress> {
        let web = self.app.web.as_ref()?;
        let port = *self.values.ports.get(&web.port)?;
        Some(WebAddress { scheme: web.scheme, port, path: web.path.clone() })
    }
}

/// `app-<id>`: the account an app runs as.
pub fn account(id: &str) -> String {
    format!("app-{id}")
}

/// An app's containers as Docker counts them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Containers {
    pub running: u32,
    /// Restarting after they ended: usually an app that keeps failing.
    pub restarting: u32,
    /// Created, exited, paused or dead.
    pub other: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    /// Some containers run, others do not.
    Partly,
    Restarting,
    Stopped,
}

impl Containers {
    pub fn state(self) -> RunState {
        if self.restarting > 0 {
            RunState::Restarting
        } else if self.running > 0 && self.other == 0 {
            RunState::Running
        } else if self.running > 0 {
            RunState::Partly
        } else {
            RunState::Stopped
        }
    }
}

/// Why a value cannot be used, for the field it was given for (e.g.
/// `ports.web`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {problem}")]
pub struct InvalidValue {
    pub field: String,
    pub problem: ValueProblem,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueProblem {
    #[error("the app has no such value")]
    Unknown,
    #[error("a value is needed")]
    Missing,
    #[error("there is no such share")]
    NoSuchShare,
    /// The app writes to it, as whoever installs it, who may only read it.
    #[error("the share can only be read")]
    NotWritable,
    #[error("not a folder's path in the share")]
    NoFolder,
    #[error("port {0} is used by zendiloOS")]
    Reserved(u16),
    #[error("the port is fixed at {0}")]
    Fixed(u16),
    #[error("port {0} is given twice")]
    Twice(u16),
    #[error("port {port} is used by {by}")]
    Taken { port: u16, by: String },
    #[error("port {0} is in use on the NAS")]
    Busy(u16),
    #[error("not a value of this kind")]
    WrongKind,
    #[error("at most {MAX_SETTING_BYTES} bytes, without line breaks")]
    TooLong,
}

/// Longest path of a folder in a share an app may be given.
pub const MAX_FOLDER_BYTES: usize = 1024;

/// A share picked for one of an app's shares, split into the share's name
/// and the folder in it (empty for the whole share): `media/Films` →
/// (`media`, `Films`).
pub fn share_and_folder(picked: &str) -> (&str, &str) {
    picked.split_once('/').unwrap_or((picked, ""))
}

/// Folder names separated by `/`: none empty, `.` or `..`, none with a
/// NUL; at most [`MAX_FOLDER_BYTES`].
fn is_folder(folder: &str) -> bool {
    folder.len() <= MAX_FOLDER_BYTES
        && folder
            .split('/')
            .all(|name| !name.is_empty() && name != "." && name != ".." && !name.contains('\0'))
}

fn invalid(field: impl Into<String>, problem: ValueProblem) -> InvalidValue {
    InvalidValue { field: field.into(), problem }
}

/// What the administrator gave, and what the NAS has, to work out an app's
/// values.
#[derive(Debug, Clone)]
pub struct ValueRequest<'a> {
    pub shares: &'a BTreeMap<String, String>,
    /// Missing ports get their default.
    pub ports: &'a BTreeMap<String, u16>,
    /// Missing settings get their default, or are generated.
    pub settings: &'a BTreeMap<String, String>,
    /// The values the app has now (an update or another try): generated
    /// settings stay, and its own ports count as free.
    pub before: Option<&'a InstallValues>,
    /// Names of the shares there are.
    pub existing_shares: &'a BTreeSet<String>,
    /// Ports other installed apps use, with their id.
    pub other_apps: &'a BTreeMap<u16, String>,
    /// Ports something listens on now.
    pub busy: &'a BTreeSet<u16>,
}

/// The values to install `app` with, checked; `generate` makes a new
/// secret for each `generate: true` setting that has none yet.
pub fn install_values(
    app: &CatalogApp,
    request: &ValueRequest<'_>,
    mut generate: impl FnMut(&Setting) -> String,
) -> Result<InstallValues, InvalidValue> {
    let declared_shares: BTreeSet<&str> = app.shares.iter().map(|s| s.id.as_str()).collect();
    if let Some(unknown) = request.shares.keys().find(|id| !declared_shares.contains(id.as_str())) {
        return Err(invalid(format!("shares.{unknown}"), ValueProblem::Unknown));
    }
    let mut shares = BTreeMap::new();
    for share in &app.shares {
        let field = format!("shares.{}", share.id);
        let picked = request.shares.get(&share.id).ok_or_else(|| invalid(&field, ValueProblem::Missing))?;
        let (name, folder) = share_and_folder(picked);
        if !request.existing_shares.contains(name) {
            return Err(invalid(&field, ValueProblem::NoSuchShare));
        }
        if picked.contains('/') && !is_folder(folder) {
            return Err(invalid(&field, ValueProblem::NoFolder));
        }
        shares.insert(share.id.clone(), picked.clone());
    }

    if let Some(unknown) = request.ports.keys().find(|id| app.ports.iter().all(|p| &p.id != *id)) {
        return Err(invalid(format!("ports.{unknown}"), ValueProblem::Unknown));
    }
    let own: BTreeSet<u16> = request.before.map(|b| b.ports.values().copied().collect()).unwrap_or_default();
    let mut ports = BTreeMap::new();
    for port in &app.ports {
        let field = format!("ports.{}", port.id);
        let chosen = request.ports.get(&port.id).copied().unwrap_or(port.default);
        if port.fixed && chosen != port.default {
            return Err(invalid(&field, ValueProblem::Fixed(port.default)));
        }
        if chosen == 0 || RESERVED_PORTS.contains(&chosen) {
            return Err(invalid(&field, ValueProblem::Reserved(chosen)));
        }
        if ports.values().any(|taken| *taken == chosen) {
            return Err(invalid(&field, ValueProblem::Twice(chosen)));
        }
        if let Some(by) = request.other_apps.get(&chosen) {
            return Err(invalid(&field, ValueProblem::Taken { port: chosen, by: by.clone() }));
        }
        if request.busy.contains(&chosen) && !own.contains(&chosen) {
            return Err(invalid(&field, ValueProblem::Busy(chosen)));
        }
        ports.insert(port.id.clone(), chosen);
    }

    if let Some(unknown) = request.settings.keys().find(|id| app.settings.iter().all(|s| &s.id != *id)) {
        return Err(invalid(format!("settings.{unknown}"), ValueProblem::Unknown));
    }
    let mut settings = BTreeMap::new();
    for setting in &app.settings {
        let field = format!("settings.{}", setting.id);
        let kept = request.before.and_then(|b| b.settings.get(&setting.id));
        let value = if setting.generate {
            kept.cloned().unwrap_or_else(|| generate(setting))
        } else {
            match request.settings.get(&setting.id) {
                Some(given) => given.trim().to_owned(),
                None => kept.cloned().or_else(|| setting.default.clone()).unwrap_or_default(),
            }
        };
        if value.len() > MAX_SETTING_BYTES || value.chars().any(char::is_control) {
            return Err(invalid(&field, ValueProblem::TooLong));
        }
        if setting.required && value.is_empty() {
            return Err(invalid(&field, ValueProblem::Missing));
        }
        let fits = value.is_empty()
            || match setting.kind {
                SettingKind::Number => value.parse::<f64>().is_ok_and(f64::is_finite),
                SettingKind::Boolean => matches!(value.as_str(), "true" | "false"),
                SettingKind::Text | SettingKind::Password => true,
            };
        if !fits {
            return Err(invalid(&field, ValueProblem::WrongKind));
        }
        settings.insert(setting.id.clone(), value);
    }
    Ok(InstallValues { shares, ports, settings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Access, Port, Setting, Share};

    fn app() -> CatalogApp {
        let mut app = crate::testing::app("immich", "Immich");
        app.ports.push(Port { id: "dns".into(), label: "DNS".into(), default: 5300, fixed: false });
        app.shares = vec![Share {
            id: "photos".into(),
            label: "Photos".into(),
            description: None,
            access: Access::ReadOnly,
        }];
        let setting = |id: &str, kind, required, generate, default: Option<&str>| Setting {
            id: id.into(),
            label: id.into(),
            description: None,
            kind,
            default: default.map(str::to_owned),
            required,
            generate,
            length: crate::domain::SECRET_LENGTH,
            alphabet: crate::domain::SecretAlphabet::Alphanumeric,
        };
        app.settings = vec![
            setting("DB_PASSWORD", SettingKind::Password, true, true, None),
            setting("WORKERS", SettingKind::Number, true, false, Some("2")),
            setting("ADMIN_EMAIL", SettingKind::Text, false, false, None),
        ];
        app
    }

    struct Given {
        shares: BTreeMap<String, String>,
        ports: BTreeMap<String, u16>,
        settings: BTreeMap<String, String>,
        existing: BTreeSet<String>,
        others: BTreeMap<u16, String>,
        busy: BTreeSet<u16>,
    }

    impl Given {
        fn new() -> Self {
            Self {
                shares: BTreeMap::from([("photos".into(), "family".into())]),
                ports: BTreeMap::new(),
                settings: BTreeMap::new(),
                existing: BTreeSet::from(["family".into()]),
                others: BTreeMap::from([(8080, "jellyfin".into())]),
                busy: BTreeSet::from([9000]),
            }
        }

        fn values(&self, before: Option<&InstallValues>) -> Result<InstallValues, InvalidValue> {
            let request = ValueRequest {
                shares: &self.shares,
                ports: &self.ports,
                settings: &self.settings,
                before,
                existing_shares: &self.existing,
                other_apps: &self.others,
                busy: &self.busy,
            };
            install_values(&app(), &request, |_| "generated".into())
        }
    }

    fn problem(result: Result<InstallValues, InvalidValue>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn defaults_fill_in_and_secrets_are_generated_once() {
        let given = Given::new();
        let values = given.values(None).unwrap();
        assert_eq!(values.ports, BTreeMap::from([("dns".into(), 5300), ("web".into(), 8096)]));
        assert_eq!(values.settings["DB_PASSWORD"], "generated");
        assert_eq!(values.settings["WORKERS"], "2");
        assert_eq!(values.settings["ADMIN_EMAIL"], "");

        let mut before = values.clone();
        before.settings.insert("DB_PASSWORD".into(), "kept".into());
        assert_eq!(given.values(Some(&before)).unwrap().settings["DB_PASSWORD"], "kept");
    }

    #[test]
    fn ports_must_be_free_and_not_zendiloos_own() {
        let mut given = Given::new();
        given.ports.insert("web".into(), 443);
        assert_eq!(problem(given.values(None)), "ports.web: port 443 is used by zendiloOS");
        given.ports.insert("web".into(), 8080);
        assert_eq!(problem(given.values(None)), "ports.web: port 8080 is used by jellyfin");
        given.ports.insert("web".into(), 5300);
        assert_eq!(problem(given.values(None)), "ports.dns: port 5300 is given twice");
        given.ports.insert("web".into(), 9000);
        assert_eq!(problem(given.values(None)), "ports.web: port 9000 is in use on the NAS");
        // Its own port is busy because it runs.
        let before =
            InstallValues { ports: BTreeMap::from([("web".into(), 9000)]), ..InstallValues::default() };
        assert!(given.values(Some(&before)).is_ok());
    }

    #[test]
    fn shares_and_settings_are_checked() {
        let mut given = Given::new();
        given.shares.insert("photos".into(), "work".into());
        assert_eq!(problem(given.values(None)), "shares.photos: there is no such share");
        given.shares.clear();
        assert_eq!(problem(given.values(None)), "shares.photos: a value is needed");
        for odd in ["family/", "family//x", "family/../etc", "family/./x", "family/a\0b"] {
            given.shares.insert("photos".into(), odd.into());
            assert_eq!(
                problem(given.values(None)),
                "shares.photos: not a folder's path in the share",
                "{odd:?}"
            );
        }
        given.shares.insert("photos".into(), "family/2024/Summer trip".into());
        assert_eq!(given.values(None).unwrap().shares["photos"], "family/2024/Summer trip");
        assert_eq!(share_and_folder("family/2024/Summer trip"), ("family", "2024/Summer trip"));
        assert_eq!(share_and_folder("family"), ("family", ""));
        given = Given::new();
        given.settings.insert("WORKERS".into(), "many".into());
        assert_eq!(problem(given.values(None)), "settings.WORKERS: not a value of this kind");
        given.settings.insert("WORKERS".into(), String::new());
        assert_eq!(problem(given.values(None)), "settings.WORKERS: a value is needed");
        given = Given::new();
        given.settings.insert("TOKEN".into(), "x".into());
        assert_eq!(problem(given.values(None)), "settings.TOKEN: the app has no such value");
    }

    #[test]
    fn containers_tell_how_the_app_runs() {
        let state = |running, restarting, other| Containers { running, restarting, other }.state();
        assert_eq!(state(2, 0, 0), RunState::Running);
        assert_eq!(state(1, 0, 1), RunState::Partly);
        assert_eq!(state(1, 1, 0), RunState::Restarting);
        assert_eq!(state(0, 0, 2), RunState::Stopped);
        assert_eq!(state(0, 0, 0), RunState::Stopped);
    }
}
