//! Taking the network over from ifupdown, and giving it back.
//!
//! zendiloosd reads ifupdown's files (as the agent hands them over, with a
//! digest), translates what it understands into systemd-networkd settings
//! and sends them. The agent first writes a record of everything it will
//! change, as it is: `/etc/network/interfaces`, `/etc/resolv.conf` (a file
//! or a link), each interface's systemd-networkd file, and whether
//! systemd-networkd and systemd-resolved were enabled and running. Only
//! then does it change anything: stop ifupdown (`networking.service`, which
//! releases DHCP leases and removes its addresses), leave it the loopback
//! only, point `/etc/resolv.conf` at systemd-resolved's stub, and start
//! systemd-resolved and systemd-networkd.
//!
//! Going back works from the record alone, from wherever the takeover got
//! to and however often it is begun: systemd-networkd drops the static
//! addresses it set (they outlive it, and ifupdown could not set them
//! again), is stopped and disabled again as it was, as is
//! systemd-resolved; the files return as they were, and
//! `networking.service` is restarted. The record goes last, so a revert
//! cut off is done again at the agent's next start. Once confirmed, the
//! record goes; people's copies of what ifupdown had stay beside the files
//! (`interfaces.zendiloos-previous`, `resolv.conf.zendiloos-previous`).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zendiloos_agent_proto::{
    ConfirmWithin, FilesDigest, Ifupdown, IfupdownFile, NetworkChange, NetworkConfig, Response,
    TakeoverConfigs,
};

use super::{MANAGER_CALL, Network, networkd_file, not_found, remove, unconfigured};
use crate::operations::OperationError;

/// The record of a takeover waiting to be confirmed, in `/etc/network`.
const RECORD: &str = "zendiloos-takeover.json";
const INTERFACES: &str = "interfaces";
const INTERFACES_D: &str = "interfaces.d";
/// Beside a file: what it was before the takeover, for people.
const KEPT: &str = ".zendiloos-previous";
/// systemd-resolved's stub, as its package links it.
const STUB: &str = "../run/systemd/resolve/stub-resolv.conf";
const MAX_FILE: u64 = 256 * 1024;
const MAX_FILES: usize = 64;
/// What ifupdown keeps once zendiloOS manages the network.
const LOOPBACK: &str = "# zendiloOS manages the network with systemd-networkd\n\
    # (/etc/systemd/network/05-zendiloos-*.network): change it in its web\n\
    # interface. What ifupdown had here is in interfaces.zendiloos-previous.\n\
    \n\
    auto lo\n\
    iface lo inet loopback\n";

/// A file as it was before the takeover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Saved {
    Missing,
    File { text: String, mode: u32 },
    Link { target: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SetUp {
    config: NetworkConfig,
    /// Its systemd-networkd file before; none: there was none.
    before: Option<String>,
}

/// Everything a takeover changes, as it was before.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    interfaces: Saved,
    resolv_conf: Saved,
    set_up: Vec<SetUp>,
    networkd_enabled: bool,
    resolved_enabled: bool,
    resolved_active: bool,
}

fn kept(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(KEPT);
    path.with_file_name(name)
}

/// `path` as it is now.
fn save(path: &Path) -> Result<Saved, OperationError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if not_found(&error) => return Ok(Saved::Missing),
        Err(error) => return Err(OperationError::Read(path.to_owned(), error)),
    };
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(path).map_err(|e| OperationError::Read(path.to_owned(), e))?;
        let target = target.to_str().ok_or_else(|| {
            OperationError::Refused(format!("{} leads to a name that is not text", path.display()))
        })?;
        return Ok(Saved::Link { target: target.to_owned() });
    }
    if !metadata.is_file() || metadata.len() > MAX_FILE {
        return Err(OperationError::Refused(format!("{} is not a file zendiloOS can keep", path.display())));
    }
    let text = fs::read_to_string(path).map_err(|e| OperationError::Read(path.to_owned(), e))?;
    Ok(Saved::File { text, mode: metadata.mode() & 0o7777 })
}

/// Makes `path` what `saved` says, atomically.
fn put(path: &Path, saved: &Saved) -> Result<(), OperationError> {
    match saved {
        Saved::Missing => remove(path),
        Saved::File { text, mode } => crate::durable::replace_mode(path, text.as_bytes(), *mode),
        Saved::Link { target } => crate::durable::link(path, target),
    }
}

/// Of ifupdown's files as read: each one's path and what was read, or why
/// not.
fn digest(files: &[IfupdownFile]) -> FilesDigest {
    let mut hash = Sha256::new();
    for file in files {
        hash.update(file.path.as_bytes());
        hash.update([0]);
        match &file.skipped {
            Some(why) => {
                hash.update(b"skipped\0");
                hash.update(why.as_bytes());
            }
            None => hash.update(file.text.as_bytes()),
        }
        hash.update([0]);
    }
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    FilesDigest::parse(&hex).unwrap_or_else(|_| unreachable!("a SHA-256 in hex"))
}

impl Network {
    fn record_path(&self) -> PathBuf {
        self.paths.ifupdown.join(RECORD)
    }

    fn interfaces_path(&self) -> PathBuf {
        self.paths.ifupdown.join(INTERFACES)
    }

    /// What `systemctl is-enabled` says of `unit`; none if it has no unit
    /// file (not installed).
    async fn enablement(&self, unit: &str) -> Option<String> {
        let output = zendiloos_exec::run(&self.systemctl, &["is-enabled", unit], super::LIMITS).await.ok()?;
        let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (!state.is_empty()).then_some(state)
    }

    async fn enabled(&self, unit: &str) -> bool {
        self.enablement(unit).await.is_some_and(|state| state == "enabled")
    }

    /// ifupdown's files, where it manages the network: `networking.service`
    /// enabled and `/etc/network/interfaces` there.
    pub(super) async fn ifupdown(&self) -> Result<Option<Ifupdown>, OperationError> {
        let there = fs::symlink_metadata(self.interfaces_path()).is_ok();
        if !there || !self.enabled("networking.service").await {
            return Ok(None);
        }
        let files = self.ifupdown_files();
        Ok(Some(Ifupdown {
            digest: digest(&files),
            files,
            active: self.active("networking.service").await,
            resolved_installed: self.enablement("systemd-resolved.service").await.is_some(),
        }))
    }

    /// `interfaces`, then the entries of `interfaces.d` by name; never
    /// through a link, which may lead out of `/etc/network`.
    fn ifupdown_files(&self) -> Vec<IfupdownFile> {
        let mut files = vec![self.ifupdown_file(INTERFACES)];
        let Ok(entries) = fs::read_dir(self.paths.ifupdown.join(INTERFACES_D)) else { return files };
        let mut names: Vec<String> =
            entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        if names.len() > MAX_FILES {
            names.truncate(MAX_FILES);
            files.push(IfupdownFile {
                path: INTERFACES_D.into(),
                text: String::new(),
                skipped: Some(format!("more than {MAX_FILES} files")),
            });
        }
        files.extend(names.iter().map(|name| self.ifupdown_file(&format!("{INTERFACES_D}/{name}"))));
        files
    }

    fn ifupdown_file(&self, path: &str) -> IfupdownFile {
        let full = self.paths.ifupdown.join(path);
        let read = match fs::symlink_metadata(&full) {
            Ok(m) if m.file_type().is_symlink() => Err("a link"),
            Ok(m) if !m.is_file() => Err("not a plain file"),
            Ok(m) if m.len() > MAX_FILE => Err("larger than 256 KiB"),
            Ok(_) => fs::read_to_string(&full).map_err(|_| "not readable as text"),
            Err(_) => Err("not readable"),
        };
        match read {
            Ok(text) => IfupdownFile { path: path.to_owned(), text, skipped: None },
            Err(why) => {
                IfupdownFile { path: path.to_owned(), text: String::new(), skipped: Some(why.into()) }
            }
        }
    }

    /// See [`zendiloos_agent_proto::Request::TakeOverNetwork`].
    pub(crate) async fn take_over(
        self: &Arc<Self>,
        configs: &TakeoverConfigs,
        seen: &FilesDigest,
        within: ConfirmWithin,
    ) -> Result<Response, OperationError> {
        let mut pending = self.pending.lock().await;
        if pending.is_some() {
            return Err(OperationError::Conflict);
        }
        let refused = |why: &str| Err(OperationError::Refused(why.to_owned()));
        if self.managed().await {
            return refused("systemd-networkd manages the network already");
        }
        if fs::symlink_metadata(self.record_path()).is_ok() {
            return refused("an earlier takeover is still being undone");
        }
        let Some(ifupdown) = self.ifupdown().await? else {
            return refused("ifupdown does not manage the network here");
        };
        if ifupdown.digest != *seen {
            return refused("ifupdown's files changed since they were read; look again");
        }
        if !ifupdown.active {
            return refused("networking.service is not running: ifupdown did not bring the network up");
        }
        if !ifupdown.resolved_installed {
            return refused("systemd-resolved is not installed");
        }
        let mut set_up = Vec::new();
        for config in configs.as_slice() {
            let file = self.file(&config.interface);
            let before = match fs::read_to_string(&file) {
                Ok(text) => Some(text),
                Err(error) if not_found(&error) => None,
                Err(error) => return Err(OperationError::Read(file, error)),
            };
            set_up.push(SetUp { config: config.clone(), before });
        }
        let record = Record {
            interfaces: save(&self.interfaces_path())?,
            resolv_conf: save(&self.paths.resolv_conf)?,
            set_up,
            networkd_enabled: self.enabled("systemd-networkd.service").await,
            resolved_enabled: self.enabled("systemd-resolved.service").await,
            resolved_active: self.active("systemd-resolved.service").await,
        };
        let text =
            serde_json::to_string_pretty(&record).map_err(|e| OperationError::Failed(e.to_string()))?;
        fs::create_dir_all(&self.paths.directory)
            .map_err(|e| OperationError::Write(self.paths.directory.clone(), e))?;
        crate::durable::replace_mode(&self.record_path(), text.as_bytes(), 0o600)?;
        if let Err(error) = self.switch(&record).await {
            tracing::error!(%error, "network: the takeover failed, ifupdown returns");
            self.undo_takeover().await;
            return Err(error);
        }
        let interfaces = configs.as_slice().iter().map(|config| config.interface.clone()).collect();
        let seconds = within.get();
        *pending = Some(self.arm(NetworkChange::Takeover { interfaces }, seconds));
        tracing::info!(seconds, "network: taken over from ifupdown, waiting for confirmation");
        Ok(Response::Done)
    }

    /// From ifupdown to systemd-networkd; the record is written already.
    async fn switch(&self, record: &Record) -> Result<(), OperationError> {
        put(&kept(&self.interfaces_path()), &record.interfaces)?;
        put(&kept(&self.paths.resolv_conf), &record.resolv_conf)?;
        for set_up in &record.set_up {
            let file = self.file(&set_up.config.interface);
            crate::durable::replace(&file, networkd_file(&set_up.config).as_bytes())?;
        }
        // ifupdown lets go: leases released, addresses removed, also of
        // the interfaces udev brought up (allow-hotplug).
        self.systemctl(&["stop", "networking.service", "ifup@*.service"]).await?;
        let mode = match &record.interfaces {
            Saved::File { mode, .. } => *mode,
            _ => 0o644,
        };
        crate::durable::replace_mode(&self.interfaces_path(), LOOPBACK.as_bytes(), mode)?;
        crate::durable::link(&self.paths.resolv_conf, STUB)?;
        self.systemctl(&["enable", "--now", "systemd-resolved.service"]).await?;
        self.systemctl(&["enable", "--now", "systemd-networkd.socket", "systemd-networkd.service"]).await
    }

    /// Once confirmed: the record goes, people's copies stay.
    pub(super) fn keep_takeover(&self) -> Result<(), OperationError> {
        remove(&self.record_path())
    }

    /// Gives the network back to ifupdown if a takeover's record is there.
    /// Failures are logged: there is nobody to tell.
    pub(super) async fn undo_takeover(&self) {
        let path = self.record_path();
        let record = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if not_found(&error) => return,
            Err(error) => {
                tracing::error!(%error, "network: cannot read the takeover's record");
                return;
            }
        };
        let record: Record = match serde_json::from_str(&record) {
            Ok(record) => record,
            Err(error) => {
                tracing::error!(%error, path = %path.display(), "network: the takeover's record cannot be read; going back by hand");
                return;
            }
        };
        tracing::warn!("network: ifupdown gets the network back");
        if !self.revert(&record).await {
            tracing::error!("network: not all of ifupdown's files are back; trying again at the next start");
            return;
        }
        for copy in [kept(&self.interfaces_path()), kept(&self.paths.resolv_conf), path] {
            if let Err(error) = remove(&copy) {
                tracing::error!(%error, "network: cannot remove what the takeover left");
            }
        }
    }

    /// Everything as the record says it was; whether the files are.
    async fn revert(&self, record: &Record) -> bool {
        let log = |done: Result<(), OperationError>, what: &str| {
            if let Err(error) = done {
                tracing::error!(%error, "network: going back, {what}");
            }
        };
        if self.managed().await {
            self.drop_static(record).await;
        }
        // Their sockets too, whichever this systemd has: one left listening
        // would start the service again.
        let stop = ["stop", "systemd-networkd.service", "systemd-networkd*.socket"];
        log(self.systemctl(&stop).await, "cannot stop systemd-networkd");
        if !record.networkd_enabled {
            let disable = ["disable", "systemd-networkd.service", "systemd-networkd.socket"];
            log(self.systemctl(&disable).await, "cannot disable systemd-networkd");
        }
        if !record.resolved_active {
            let stop = ["stop", "systemd-resolved.service", "systemd-resolved*.socket"];
            log(self.systemctl(&stop).await, "cannot stop systemd-resolved");
        }
        if !record.resolved_enabled {
            log(
                self.systemctl(&["disable", "systemd-resolved.service"]).await,
                "cannot disable systemd-resolved",
            );
        }
        let mut restored = true;
        let mut back = |done: Result<(), OperationError>, what: &str| {
            if let Err(error) = done {
                tracing::error!(%error, "network: cannot put {what} back");
                restored = false;
            }
        };
        for set_up in &record.set_up {
            let file = self.file(&set_up.config.interface);
            match &set_up.before {
                Some(text) => {
                    back(crate::durable::replace(&file, text.as_bytes()), "a systemd-networkd file")
                }
                None => back(remove(&file), "a systemd-networkd file"),
            }
        }
        back(put(&self.paths.resolv_conf, &record.resolv_conf), "/etc/resolv.conf");
        back(put(&self.interfaces_path(), &record.interfaces), "/etc/network/interfaces");
        log(self.systemctl(&["restart", "networking.service"]).await, "ifupdown did not come up");
        restored
    }

    /// Has systemd-networkd set up the interfaces without IPv4 addresses and
    /// waits until it has, ten seconds at most: the static ones it set stay
    /// when it stops, and ifupdown could not set them again.
    async fn drop_static(&self, record: &Record) {
        for set_up in &record.set_up {
            let file = self.file(&set_up.config.interface);
            if let Err(error) =
                crate::durable::replace(&file, networkd_file(&unconfigured(&set_up.config)).as_bytes())
            {
                tracing::error!(%error, "network: going back, cannot write a file without addresses");
            }
        }
        if let Err(error) = crate::pools::run(&self.busctl, &[MANAGER_CALL, &["Reload"]].concat()).await {
            tracing::error!(%error, "network: going back, systemd-networkd did not reload");
            return;
        }
        let mut objects = Vec::new();
        for set_up in &record.set_up {
            let Some((index, object)) = self.link(&set_up.config.interface).await else { continue };
            let index = index.to_string();
            let call = [MANAGER_CALL, &["ReconfigureLink", "i", index.as_str()]].concat();
            match crate::pools::run(&self.busctl, &call).await {
                Ok(()) if !object.is_empty() => objects.push(object),
                Ok(()) => {}
                Err(error) => tracing::error!(%error, "network: going back, a link was not reconfigured"),
            }
        }
        if objects.is_empty() {
            return;
        }
        // It begins at once; then each link says when it is done.
        tokio::time::sleep(Duration::from_secs(1)).await;
        for _ in 0..20 {
            let mut settled = true;
            for object in &objects {
                let asked = [
                    "get-property",
                    "org.freedesktop.network1",
                    object.as_str(),
                    "org.freedesktop.network1.Link",
                    "AdministrativeState",
                ];
                let state = zendiloos_exec::run(&self.busctl, &asked, super::LIMITS).await;
                let state =
                    state.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
                settled &= !state.contains("\"configuring\"") && !state.contains("\"pending\"");
            }
            if settled {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tracing::warn!("network: going back, systemd-networkd did not finish dropping the addresses");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_kept_as_they_are_files_links_or_none() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("resolv.conf");
        assert_eq!(save(&file).unwrap(), Saved::Missing);
        fs::write(&file, "nameserver 192.168.1.1\n").unwrap();
        let saved = save(&file).unwrap();
        assert!(
            matches!(&saved, Saved::File { text, .. } if text == "nameserver 192.168.1.1\n"),
            "{saved:?}"
        );

        let link = folder.path().join("link");
        std::os::unix::fs::symlink("../run/resolvconf/resolv.conf", &link).unwrap();
        let linked = save(&link).unwrap();
        assert_eq!(linked, Saved::Link { target: "../run/resolvconf/resolv.conf".into() });

        // Back as they were: a link where a file was, and the other way.
        put(&file, &linked).unwrap();
        assert_eq!(fs::read_link(&file).unwrap(), Path::new("../run/resolvconf/resolv.conf"));
        put(&file, &saved).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "nameserver 192.168.1.1\n");
        put(&file, &Saved::Missing).unwrap();
        put(&file, &Saved::Missing).unwrap();
        assert!(fs::symlink_metadata(&file).is_err());
        assert!(save(folder.path()).is_err(), "a folder is no file to keep");
        assert_eq!(kept(&file), folder.path().join("resolv.conf.zendiloos-previous"));
    }

    #[test]
    fn the_digest_changes_with_any_file() {
        let file =
            |path: &str, text: &str| IfupdownFile { path: path.into(), text: text.into(), skipped: None };
        let one = digest(&[file("interfaces", "auto lo\n")]);
        assert_eq!(one, digest(&[file("interfaces", "auto lo\n")]));
        assert_ne!(one, digest(&[file("interfaces", "auto lo \n")]));
        assert_ne!(one, digest(&[file("interfaces", "auto lo\n"), file("interfaces.d/x", "")]));
        let skipped = IfupdownFile { skipped: Some("a link".into()), ..file("interfaces.d/x", "") };
        assert_ne!(digest(&[file("interfaces.d/x", "")]), digest(&[skipped]));
    }
}
