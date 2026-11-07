//! Apps installed from a catalog the earlier way, run by Docker Compose from
//! the file the agent kept: what travels to the agents to run and remove
//! one, and the folders and devices of projects and the App Center's
//! templates. These types only keep what reaches the agents within bounds.

use serde::{Deserialize, Serialize};

use crate::is_file_name;

/// The id of an installed app: its catalog id, its Compose project, and
/// with `app-` in front its Linux account. Two to 28 characters (the
/// account name has at most 32): lowercase letters and digits in parts
/// joined by single `-`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AppName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid app id")]
pub struct InvalidAppName;

impl AppName {
    pub const MAX_LEN: usize = 28;
    /// What the account name starts with.
    pub const ACCOUNT_PREFIX: &str = "app-";

    pub fn parse(name: &str) -> Result<Self, InvalidAppName> {
        let valid = (2..=Self::MAX_LEN).contains(&name.len())
            && name.split('-').all(|part| {
                !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            });
        if valid { Ok(Self(name.to_owned())) } else { Err(InvalidAppName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The Linux user and group the app runs as: `app-<id>`.
    pub fn account(&self) -> String {
        format!("{}{}", Self::ACCOUNT_PREFIX, self.0)
    }
}

impl TryFrom<String> for AppName {
    type Error = InvalidAppName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<AppName> for String {
    fn from(name: AppName) -> Self {
        name.0
    }
}

/// A folder below an app's data folder or a share, relative to it: names
/// separated by `/`, none empty, `.` or `..`; at most 1024 bytes. Empty
/// means that folder itself.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AppFolder(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a valid folder of an app")]
pub struct InvalidAppFolder;

impl AppFolder {
    pub const MAX_BYTES: usize = 1024;
    /// Most folders one app may have.
    pub const MAX_COUNT: usize = 64;

    pub fn parse(value: &str) -> Result<Self, InvalidAppFolder> {
        let valid =
            value.is_empty() || (value.len() <= Self::MAX_BYTES && value.split('/').all(is_file_name));
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidAppFolder) }
    }

    /// The folder itself: empty.
    pub fn whole() -> Self {
        Self(String::new())
    }

    pub fn is_whole(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|name| !name.is_empty())
    }
}

impl TryFrom<String> for AppFolder {
    type Error = InvalidAppFolder;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<AppFolder> for String {
    fn from(folder: AppFolder) -> Self {
        folder.0
    }
}

/// Most lines of an app's log [`crate::Request::AppLogs`] returns.
pub const MAX_LOG_LINES: u32 = 5000;

/// A device of the machine an app may be given, such as `/dev/dri` or
/// `/dev/serial/by-id/usb-…`: below `/dev`, names of letters, digits and
/// `.`, `_`, `-`, `:`, `+`, `@`, none `.` or `..`; at most 255 bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DevicePath(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a device path below /dev")]
pub struct InvalidDevicePath;

impl DevicePath {
    pub const MAX_BYTES: usize = 255;
    /// Most devices one request asks about.
    pub const MAX_COUNT: usize = 64;

    pub fn parse(value: &str) -> Result<Self, InvalidDevicePath> {
        let valid = value.len() <= Self::MAX_BYTES
            && value.strip_prefix("/dev/").is_some_and(|inside| {
                inside.split('/').all(|name| {
                    !name.is_empty()
                        && name != "."
                        && name != ".."
                        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-:+@".contains(&b))
                })
            });
        if valid { Ok(Self(value.to_owned())) } else { Err(InvalidDevicePath) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path below `/dev`: `dri`, `serial/by-id/…`.
    pub fn inside(&self) -> &str {
        &self.0["/dev/".len()..]
    }
}

impl TryFrom<String> for DevicePath {
    type Error = InvalidDevicePath;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<DevicePath> for String {
    fn from(path: DevicePath) -> Self {
        path.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_fit_an_account_name() {
        for good in ["jellyfin", "uptime-kuma", "2fauth", "a1", &"a".repeat(28)] {
            let name = AppName::parse(good).unwrap();
            assert!(name.account().len() <= 32, "{good}");
        }
        for bad in ["", "a", "Jellyfin", "-x", "x-", "a--b", "a_b", "a.b", &"a".repeat(29)] {
            assert!(AppName::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(AppName::parse("uptime-kuma").unwrap().account(), "app-uptime-kuma");
    }

    #[test]
    fn folders_stay_below_the_data_folder() {
        assert!(AppFolder::parse("").is_ok());
        assert_eq!(
            AppFolder::parse("config/cache").unwrap().names().collect::<Vec<_>>(),
            ["config", "cache"]
        );
        for bad in ["/config", "config/", "../x", "a/./b", "a//b", "a\0b"] {
            assert!(AppFolder::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn devices_lie_below_dev() {
        for good in
            ["/dev/dri", "/dev/ttyUSB0", "/dev/net/tun", "/dev/serial/by-id/usb-ITead_Sonoff-if00:x+y@z"]
        {
            let device = DevicePath::parse(good).unwrap();
            assert_eq!(String::from(device.clone()), good);
            assert_eq!(device.inside(), &good[5..]);
        }
        for bad in
            ["/dev/", "/dev", "/etc/passwd", "/dev/../etc", "/dev/a//b", "/dev/a b", "dev/dri", "/dev/./x"]
        {
            assert!(DevicePath::parse(bad).is_err(), "{bad}");
        }
        assert!(DevicePath::parse(&format!("/dev/{}", "a".repeat(251))).is_err());
        assert!(serde_json::from_str::<DevicePath>("\"/dev/../x\"").is_err());
    }
}
