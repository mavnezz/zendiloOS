//! The Docker engine as administrators see it: containers, images,
//! networks and Compose projects, whoever made them. Administrators may run
//! any Compose file (docs/architecture.md, 4.9); what reaches a command line
//! is still checked here, as everything else.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The longest Docker works on one request once its turn came: pulling an
/// image, starting containers, and a project's deploy with its pull and its
/// containers made together, in one budget.
pub const DOCKER_WORK: Duration = Duration::from_secs(30 * 60);
/// What a request about Docker takes at most besides that work: systemd's
/// units that pin a project's folders, before and after Compose, and quick
/// looks at what its containers mount.
pub const DOCKER_AROUND: Duration = Duration::from_secs(20 * 60);
/// How long a client waits for a request about Docker: the agent waits for
/// its turn half of what the client waits at most, so the other half holds
/// the work and what is around it.
pub const DOCKER_PATIENCE: Duration = Duration::from_secs(2 * (30 + 20) * 60);
const _: () = assert!(DOCKER_PATIENCE.as_secs() >= 2 * (DOCKER_WORK.as_secs() + DOCKER_AROUND.as_secs()));

/// A text the agent passes to `docker` as one argument, checked by `valid`.
macro_rules! checked_text {
    ($(#[$doc:meta])* $name:ident, $error:literal, $valid:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn parse(text: &str) -> Result<Self, InvalidDockerName> {
                let valid: fn(&str) -> bool = $valid;
                if valid(text) { Ok(Self(text.to_owned())) } else { Err(InvalidDockerName($error)) }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = InvalidDockerName;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid {0}")]
pub struct InvalidDockerName(pub &'static str);

fn hex_id(text: &str) -> bool {
    (12..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// What Docker allows in a container or network name; never an option.
fn docker_name(text: &str, max: usize) -> bool {
    let mut bytes = text.bytes();
    (1..=max).contains(&text.len())
        && bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && bytes.all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

checked_text!(
    /// A container's ID (12 to 64 hex digits) or name.
    ContainerId,
    "container",
    |text| hex_id(text) || docker_name(text, 128)
);

checked_text!(
    /// An image's ID, `sha256:` and 64 hex digits, or 12 to 64 hex digits.
    ImageId,
    "image ID",
    |text| hex_id(text.strip_prefix("sha256:").unwrap_or(text))
);

checked_text!(
    /// An image to pull: `postgres:17`, `ghcr.io/owner/app@sha256:…`.
    ImageReference,
    "image reference",
    |text| {
        (1..=255).contains(&text.len())
            && text.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
            && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/:@-".contains(&b))
    }
);

checked_text!(
    /// A network's name.
    NetworkName,
    "network name",
    |text| docker_name(text, 64)
);

checked_text!(
    /// A Compose project's name, as Compose allows it: lowercase letters,
    /// digits, `-` and `_`, starting with a letter or digit.
    ProjectName,
    "project name",
    |text| {
        let mut bytes = text.bytes();
        (1..=63).contains(&text.len())
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    }
);

checked_text!(
    /// A volume's name as Docker allows it (a letter or digit, then
    /// letters, digits, `_`, `.` and `-`), at most 200 characters, so that
    /// `<name>-YYYYMMDD-HHMMSS.tar.zst` stays a file name. Docker's
    /// anonymous volumes have 64 hex digits.
    VolumeName,
    "volume name",
    |text| docker_name(text, 200)
);

checked_text!(
    /// The name a backup's files get, without `.tar.zst` and `.json`:
    /// letters, digits, `_`, `.` and `-`, not starting with `.` or `-`, at
    /// most 230 characters (`teslamate_db-20261108-030000`).
    BackupStem,
    "backup name",
    |text| docker_name(text, 230)
);

checked_text!(
    /// An archive's SHA-256: 64 lowercase hex digits.
    ArchiveDigest,
    "SHA-256",
    |text| text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
);

/// The labels a volume is made with: at most 64, each key 1 to 255 bytes
/// without `=`, each value at most 4096 bytes, no control characters in
/// either. They reach `docker volume create` as `--label=<key>=<value>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BTreeMap<String, String>", into = "BTreeMap<String, String>")]
pub struct VolumeLabels(BTreeMap<String, String>);

impl VolumeLabels {
    pub const MAX_LABELS: usize = 64;

    pub fn parse(labels: BTreeMap<String, String>) -> Result<Self, InvalidDockerName> {
        let printable = |text: &str| !text.chars().any(char::is_control);
        let valid = labels.len() <= Self::MAX_LABELS
            && labels.iter().all(|(key, value)| {
                (1..=255).contains(&key.len())
                    && !key.contains('=')
                    && printable(key)
                    && value.len() <= 4096
                    && printable(value)
            });
        if valid { Ok(Self(labels)) } else { Err(InvalidDockerName("set of volume labels")) }
    }

    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }
}

impl TryFrom<BTreeMap<String, String>> for VolumeLabels {
    type Error = InvalidDockerName;

    fn try_from(value: BTreeMap<String, String>) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<VolumeLabels> for BTreeMap<String, String> {
    fn from(value: VolumeLabels) -> Self {
        value.0
    }
}

/// A share by the volume it is on and its name: `/volume<N>/<share>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareLocation {
    pub volume: crate::VolumeNumber,
    pub share: crate::ShareName,
}

/// A file a backup wrote into a share: where, how large, and which file it
/// is (device and inode), so that only that file is ever removed again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WrittenFile {
    pub path: crate::SharePath,
    pub bytes: u64,
    pub device: u64,
    pub inode: u64,
    /// When it was written, in nanoseconds since 1970: with its inode and
    /// size, which file it is (the device number of a ZFS dataset changes
    /// from one mount to the next).
    pub modified: i64,
}

/// A backup found in a folder of a share, to be recorded: its archive and
/// manifest, both writable by root only, the archive's SHA-256 as the
/// manifest says and as it was hashed, and what the manifest says of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundBackup {
    pub volume: VolumeName,
    pub archive: WrittenFile,
    pub manifest: WrittenFile,
    pub sha256: ArchiveDigest,
    pub data_bytes: u64,
    /// Unknown for archives written before entries were counted.
    pub entries: Option<u64>,
    /// Unix seconds, as the manifest says.
    pub backed_up_at: i64,
    pub manifest_json: String,
}

/// A file that looked like a backup's archive and is not recorded, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefusedBackupFile {
    pub path: crate::SharePath,
    pub reason: String,
}

/// What one volume's data takes up, as counted in Docker's data root;
/// `complete` is false where counting stopped at its limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeSize {
    pub name: String,
    pub bytes: u64,
    pub complete: bool,
}

/// Whether `text` is an absolute path below a volume, `/volume<N>/…`, with
/// no `.` or `..` and nothing that is not printable.
fn below_volume(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("/volume") else { return false };
    let mut parts = rest.split('/');
    let number = parts.next().unwrap_or_default();
    !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
        && text.len() <= 4096
        && !text.chars().any(char::is_control)
        && parts.clone().count() >= 1
        && parts.all(|part| !part.is_empty() && part != "." && part != "..")
}

checked_text!(
    /// The folder of a project of an administrator's, below a volume:
    /// relative paths in its Compose file start there.
    ProjectDirectory,
    "project folder",
    below_volume
);

checked_text!(
    /// An IPv4 or IPv6 network for a Docker network: `172.30.0.0/24`.
    Subnet,
    "subnet",
    |text| {
        let Some((address, prefix)) = text.split_once('/') else { return false };
        let Ok(address) = address.parse::<IpAddr>() else { return false };
        let maximum = if address.is_ipv4() { 32 } else { 128 };
        prefix.bytes().all(|b| b.is_ascii_digit())
            && prefix.parse::<u8>().is_ok_and(|prefix| (8..=maximum).contains(&prefix))
    }
);

/// A Compose file as written, at most 256 KiB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ComposeText(String);

impl ComposeText {
    pub const MAX_BYTES: usize = 256 * 1024;

    pub fn parse(text: &str) -> Result<Self, InvalidDockerName> {
        if text.len() <= Self::MAX_BYTES && !text.contains('\0') {
            Ok(Self(text.to_owned()))
        } else {
            Err(InvalidDockerName("Compose file of at most 256 KiB"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ComposeText {
    type Error = InvalidDockerName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ComposeText> for String {
    fn from(value: ComposeText) -> Self {
        value.0
    }
}

/// An administrator's project as Compose normalized it when they confirmed
/// it (`docker compose config --format json`): a JSON object with its
/// services, at most 1 MiB. Its paths are absolute; nothing else is read
/// to run it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProjectConfig(String);

impl ProjectConfig {
    pub const MAX_BYTES: usize = 1024 * 1024;

    pub fn parse(text: &str) -> Result<Self, InvalidDockerName> {
        let valid = text.len() <= Self::MAX_BYTES
            && serde_json::from_str::<serde_json::Value>(text)
                .is_ok_and(|value| value.get("services").is_some_and(serde_json::Value::is_object));
        if valid { Ok(Self(text.to_owned())) } else { Err(InvalidDockerName("normalized Compose project")) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProjectConfig {
    type Error = InvalidDockerName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ProjectConfig> for String {
    fn from(value: ProjectConfig) -> Self {
        value.0
    }
}

/// What a terminal in a container runs: a program and its arguments, as an
/// administrator typed them, split at spaces (`/bin/bash`, `sh -l`). Never
/// a `docker exec` option: the first word does not start with `-`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct ExecCommand(Vec<String>);

impl ExecCommand {
    pub const MAX_WORDS: usize = 16;
    pub const MAX_WORD_BYTES: usize = 256;

    pub fn parse(words: Vec<String>) -> Result<Self, InvalidDockerName> {
        let valid = (1..=Self::MAX_WORDS).contains(&words.len())
            && words.first().is_some_and(|first| !first.starts_with('-'))
            && words.iter().all(|word| {
                (1..=Self::MAX_WORD_BYTES).contains(&word.len())
                    && !word.chars().any(|c| c.is_control() || c.is_whitespace())
            });
        if valid { Ok(Self(words)) } else { Err(InvalidDockerName("command of up to 16 words")) }
    }

    /// The words of `line`, split at whitespace.
    pub fn from_line(line: &str) -> Result<Self, InvalidDockerName> {
        Self::parse(line.split_whitespace().map(str::to_owned).collect())
    }

    pub fn words(&self) -> &[String] {
        &self.0
    }

    /// As typed, the words joined by spaces.
    pub fn line(&self) -> String {
        self.0.join(" ")
    }
}

impl TryFrom<Vec<String>> for ExecCommand {
    type Error = InvalidDockerName;

    fn try_from(value: Vec<String>) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ExecCommand> for Vec<String> {
    fn from(value: ExecCommand) -> Self {
        value.0
    }
}

/// A terminal's size in characters, each side 1 to 1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
}

impl TerminalSize {
    pub const MAX: u16 = 1000;

    /// Within 1 to [`TerminalSize::MAX`] on each side.
    pub fn clamped(columns: u16, rows: u16) -> Self {
        Self { columns: columns.clamp(1, Self::MAX), rows: rows.clamp(1, Self::MAX) }
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self { columns: 80, rows: 24 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    /// Only a container that is not running.
    Remove,
}

impl ContainerAction {
    pub fn docker_command(self) -> &'static str {
        match self {
            ContainerAction::Start => "start",
            ContainerAction::Stop => "stop",
            ContainerAction::Restart => "restart",
            ContainerAction::Remove => "rm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAction {
    /// Starts the containers there are.
    Start,
    Stop,
    Restart,
    /// Creates and starts the containers from the Compose file, recreating
    /// those whose configuration or image changed.
    Up,
    /// Fetches the images the Compose file names.
    Pull,
    /// Removes the containers and networks; images and data stay.
    Down,
}

impl ProjectAction {
    /// Needs the project's Compose file.
    pub fn needs_file(self) -> bool {
        matches!(self, ProjectAction::Up | ProjectAction::Pull)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_never_become_options_or_paths() {
        for good in ["d853eed323e7", "uptime-kuma-uptime-kuma-1", "teslamate_db"] {
            assert!(ContainerId::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "-f", "--rm", "a/b", "a b", "../x", &"x".repeat(129)] {
            assert!(ContainerId::parse(bad).is_err(), "{bad}");
        }
        assert!(
            ImageId::parse("sha256:0772aa7e48ba0ed3cec5277b1df78865b7875fd275251d0797f0e793f0819912").is_ok()
        );
        assert!(ImageId::parse("sha256:xyz").is_err());
        assert!(ImageReference::parse("ghcr.io/maintainerr/maintainerr:latest").is_ok());
        assert!(ImageReference::parse("postgres@sha256:abc").is_ok());
        assert!(ImageReference::parse("-postgres").is_err());
        assert!(ImageReference::parse("postgres latest").is_err());
        assert!(NetworkName::parse("proxy_net").is_ok() && NetworkName::parse("-net").is_err());
        assert!(ProjectName::parse("teslamate").is_ok() && ProjectName::parse("Tesla").is_err());
    }

    #[test]
    fn volumes_their_labels_and_backups_are_checked() {
        for good in ["teslamate_db", "a1", &"f".repeat(64), "media.cache-2"] {
            assert!(VolumeName::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "-v", ".hidden", "a/b", "../x", "a b", &"x".repeat(201)] {
            assert!(VolumeName::parse(bad).is_err(), "{bad}");
        }
        assert!(BackupStem::parse("teslamate_db-20261108-030000").is_ok());
        assert!(BackupStem::parse("x/../y").is_err() && BackupStem::parse(".x").is_err());
        assert!(ArchiveDigest::parse(&"ab".repeat(32)).is_ok());
        assert!(ArchiveDigest::parse(&"AB".repeat(32)).is_err() && ArchiveDigest::parse("abc").is_err());

        let labels = |pairs: &[(&str, &str)]| {
            VolumeLabels::parse(pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect())
        };
        let good = labels(&[("com.docker.compose.project", "tesla"), ("note", "a = b")]).unwrap();
        assert_eq!(good.as_map().len(), 2);
        for bad in [&[("", "x")][..], &[("a=b", "x")], &[("a", "x\ny")], &[("a\u{7}", "x")]] {
            assert!(labels(bad).is_err(), "{bad:?}");
        }
        let many: BTreeMap<String, String> = (0..65).map(|n| (format!("k{n}"), String::new())).collect();
        assert!(VolumeLabels::parse(many).is_err());
        let json = serde_json::to_string(&good).unwrap();
        assert_eq!(serde_json::from_str::<VolumeLabels>(&json).unwrap(), good);
        assert!(serde_json::from_str::<VolumeLabels>(r#"{"a=b":"c"}"#).is_err());
    }

    #[test]
    fn commands_are_words_and_never_options() {
        let bash = ExecCommand::from_line("  /bin/bash  -l ").unwrap();
        assert_eq!(bash.words(), ["/bin/bash", "-l"]);
        assert_eq!(bash.line(), "/bin/bash -l");
        for bad in ["", "   ", "--privileged sh", "-u root", "sh\u{0}", &"x".repeat(257)] {
            assert!(ExecCommand::from_line(bad).is_err(), "{bad:?}");
        }
        assert!(ExecCommand::parse(vec!["sh".into(), "a b".into()]).is_err(), "no blanks within a word");
        assert!(ExecCommand::parse(vec!["sh".into(), "\n".into()]).is_err());
        assert!(ExecCommand::from_line(&vec!["a"; 17].join(" ")).is_err());
        let json = serde_json::to_string(&bash).unwrap();
        assert_eq!(json, r#"["/bin/bash","-l"]"#);
        assert!(serde_json::from_str::<ExecCommand>(r#"["-it"]"#).is_err());
        assert_eq!(TerminalSize::clamped(0, 5000), TerminalSize { columns: 1, rows: 1000 });
    }

    #[test]
    fn project_folders_lie_below_a_volume() {
        assert!(ProjectDirectory::parse("/volume12/docker/teslamate").is_ok());
        for bad in [
            "/etc/zendiloos/apps",
            "/volume1/../etc",
            "/volume1/docker/./x",
            "/volumeX/docker",
            "/volume1//docker",
            "volume1/docker",
        ] {
            assert!(ProjectDirectory::parse(bad).is_err(), "{bad}");
        }
        assert!(ProjectDirectory::parse("/volume1").is_err(), "a volume itself is no project folder");
    }

    #[test]
    fn project_configs_are_compose_objects() {
        assert!(ProjectConfig::parse(r#"{"name":"x","services":{"web":{"image":"nginx"}}}"#).is_ok());
        for bad in ["", "services: {}", "[]", r#"{"name":"x"}"#, r#"{"services":[]}"#] {
            assert!(ProjectConfig::parse(bad).is_err(), "{bad}");
        }
        let large = format!(r#"{{"services":{{}},"x":"{}"}}"#, "a".repeat(ProjectConfig::MAX_BYTES));
        assert!(ProjectConfig::parse(&large).is_err());
    }

    #[test]
    fn subnets_are_networks_with_a_prefix() {
        assert!(Subnet::parse("172.30.0.0/24").is_ok());
        assert!(Subnet::parse("fd00:1::/64").is_ok());
        for bad in ["172.30.0.0", "172.30.0.0/33", "172.30.0.0/4", "nas/24", "172.30.0.0/+8"] {
            assert!(Subnet::parse(bad).is_err(), "{bad}");
        }
        assert!(ComposeText::parse(&"x".repeat(ComposeText::MAX_BYTES + 1)).is_err());
    }
}
