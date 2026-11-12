//! An app as a catalog offers it: what it is, what it needs from the NAS,
//! and its Compose file. Read from an index and checked against the
//! catalog's format and policy before it gets here.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebScheme {
    Http,
    Https,
}

/// The interface zendiloOS opens for the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Web {
    /// The id of one of the app's ports.
    pub port: String,
    pub path: String,
    pub scheme: WebScheme,
}

/// A host port the app publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub id: String,
    pub label: String,
    /// The host port zendiloOS proposes.
    pub default: u16,
    /// The app listens on this port itself (host network): it cannot move.
    pub fixed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

/// A share the administrator picks for the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub access: Access,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Text,
    Password,
    Number,
    Boolean,
}

/// A value the administrator enters, or zendiloOS generates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub kind: SettingKind,
    /// As text; never for passwords.
    pub default: Option<String>,
    pub required: bool,
    /// zendiloOS generates it once, when installing; nobody is asked.
    pub generate: bool,
    /// How many characters a generated value has.
    pub length: u8,
    pub alphabet: SecretAlphabet,
}

/// The characters a generated value is made of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SecretAlphabet {
    /// `A–Z`, `a–z`, `0–9`.
    #[default]
    Alphanumeric,
    /// `0–9`, `a–f`: for keys an app reads as hexadecimal.
    Hex,
}

/// The length of a generated value the catalog does not set.
pub const SECRET_LENGTH: u8 = 32;

/// Exceptions from the policy the app declares: shown before it is filled
/// in, and confirmed as the project's risks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    pub devices: Vec<String>,
    pub capabilities: Vec<String>,
    pub host_network: bool,
    /// The Docker socket: control over every container, and so over the
    /// whole NAS.
    pub docker_socket: bool,
}

/// The texts of one language; each falls back to English.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Translation {
    pub name: Option<String>,
    pub tagline: Option<String>,
    pub description: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogApp {
    pub id: String,
    pub name: String,
    /// The app's own version, for apps whose images name one; most follow
    /// a moving tag and name none.
    pub version: Option<String>,
    pub tagline: String,
    pub description: String,
    pub categories: Vec<String>,
    pub developer: String,
    pub website: String,
    /// Where the app's code is.
    pub source_code: String,
    pub license: String,
    /// Debian's (and Docker's) names of the architectures its images have
    /// (`amd64`, `arm64`), which the catalog checks against the registries.
    pub architectures: Vec<String>,
    pub notes: Option<String>,
    pub web: Option<Web>,
    pub ports: Vec<Port>,
    pub shares: Vec<Share>,
    pub settings: Vec<Setting>,
    pub permissions: Permissions,
    /// By ISO 639-1 language.
    pub translations: BTreeMap<String, Translation>,
    /// The manifest as the index has it, as JSON: the agent reads and
    /// checks it again itself.
    pub manifest: String,
    /// The Compose file as written; filled in on the NAS.
    pub compose: String,
    /// The icon, relative to the index.
    pub icon: String,
    /// Changes whenever any file of the app changes: an update.
    pub digest: String,
}

impl CatalogApp {
    /// Its images run on a machine of `architecture`: the catalog lists it.
    pub fn runs_on(&self, architecture: &str) -> bool {
        self.architectures.iter().any(|listed| listed == architecture)
    }
}
