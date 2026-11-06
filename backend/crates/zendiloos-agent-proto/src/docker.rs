//! The Docker engine as administrators see it: containers, images,
//! networks and Compose projects, whoever made them. Administrators may run
//! any Compose file (docs/architecture.md, 4.9); what reaches a command line
//! is still checked here, as everything else.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

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
