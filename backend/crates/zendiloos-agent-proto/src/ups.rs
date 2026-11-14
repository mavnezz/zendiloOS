//! A UPS through Network UPS Tools (NUT): where it is, when the NAS shuts
//! down on battery, and whether other devices may follow it. Everything
//! here ends up in NUT's configuration files, so every value is checked
//! while it is read: none can carry a line break, a quote or a comment
//! sign into them.

use serde::{Deserialize, Serialize};

/// The name the UPS has in NUT, here and for the devices that follow it:
/// always the same, which is also the one other NAS systems look for.
pub const UPS_NAME: &str = "ups";
/// The port NUT's server listens on.
pub const UPS_PORT: u16 = 3493;
/// The account the monitor on this machine signs in to its own server with.
pub const UPS_LOCAL_USER: &str = "upsmon_local";

/// A word in one of NUT's files: an account's name, a password, an SNMP
/// community, a serial number. Letters, digits and a few signs, at most 64;
/// no blank, quote, backslash, `#` or `=`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UpsWord(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("1 to 64 letters, digits or one of . _ - + : , / @ % ! ~ ^ * ( )")]
pub struct InvalidUpsWord;

impl UpsWord {
    pub fn parse(word: &str) -> Result<Self, InvalidUpsWord> {
        let allowed = |c: char| c.is_ascii_alphanumeric() || "._-+:,/@%!~^*()".contains(c);
        if !word.is_empty() && word.len() <= 64 && word.chars().all(allowed) {
            Ok(Self(word.to_owned()))
        } else {
            Err(InvalidUpsWord)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for UpsWord {
    type Error = InvalidUpsWord;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<UpsWord> for String {
    fn from(word: UpsWord) -> Self {
        word.0
    }
}

/// A host NUT connects to: a name or an IPv4 address. No IPv6 address,
/// which NUT writes in another way (`ups@[::1]:3493`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UpsHost(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a host name or an IPv4 address")]
pub struct InvalidUpsHost;

impl UpsHost {
    pub fn parse(host: &str) -> Result<Self, InvalidUpsHost> {
        let label = |label: &str| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        };
        if !host.is_empty() && host.len() <= 253 && host.split('.').all(label) {
            Ok(Self(host.to_ascii_lowercase()))
        } else {
            Err(InvalidUpsHost)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for UpsHost {
    type Error = InvalidUpsHost;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<UpsHost> for String {
    fn from(host: UpsHost) -> Self {
        host.0
    }
}

/// A USB vendor or product id: four hexadecimal digits, kept in lowercase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UsbId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("four hexadecimal digits")]
pub struct InvalidUsbId;

impl UsbId {
    pub fn parse(id: &str) -> Result<Self, InvalidUsbId> {
        if id.len() == 4 && id.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(Self(id.to_ascii_lowercase()))
        } else {
            Err(InvalidUsbId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for UsbId {
    type Error = InvalidUsbId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<UsbId> for String {
    fn from(id: UsbId) -> Self {
        id.0
    }
}

/// NUT's drivers for a UPS on USB, as `nut-scanner` names them. Nearly
/// every current UPS speaks USB HID (`usbhid-ups`); the others are for
/// older ones and those with a serial converter built in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsbDriver {
    #[serde(rename = "usbhid-ups")]
    UsbHid,
    #[serde(rename = "nutdrv_qx")]
    Qx,
    #[serde(rename = "blazer_usb")]
    Blazer,
    #[serde(rename = "bcmxcp_usb")]
    BcmXcp,
    #[serde(rename = "tripplite_usb")]
    TrippLite,
    #[serde(rename = "richcomm_usb")]
    Richcomm,
    #[serde(rename = "riello_usb")]
    Riello,
    #[serde(rename = "nutdrv_atcl_usb")]
    Atcl,
}

impl UsbDriver {
    pub const ALL: [Self; 8] = [
        Self::UsbHid,
        Self::Qx,
        Self::Blazer,
        Self::BcmXcp,
        Self::TrippLite,
        Self::Richcomm,
        Self::Riello,
        Self::Atcl,
    ];

    /// The driver's name in `ups.conf`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UsbHid => "usbhid-ups",
            Self::Qx => "nutdrv_qx",
            Self::Blazer => "blazer_usb",
            Self::BcmXcp => "bcmxcp_usb",
            Self::TrippLite => "tripplite_usb",
            Self::Richcomm => "richcomm_usb",
            Self::Riello => "riello_usb",
            Self::Atcl => "nutdrv_atcl_usb",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|driver| driver.as_str() == name)
    }
}

/// The SNMP versions offered: both name a community and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnmpVersion {
    V1,
    V2c,
}

impl SnmpVersion {
    /// As `snmp-ups` wants it in `ups.conf`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::V1 => "v1",
            Self::V2c => "v2c",
        }
    }
}

/// Where the UPS is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpsSource {
    /// Plugged into this machine. Without ids, the driver takes the first
    /// UPS it finds; with them, the one they name.
    Usb { driver: UsbDriver, vendor_id: Option<UsbId>, product_id: Option<UsbId>, serial: Option<UpsWord> },
    /// A UPS with a network card, asked over SNMP (`snmp-ups`).
    Snmp { address: UpsHost, community: UpsWord, version: SnmpVersion },
    /// The UPS of another device, which runs NUT's server: this machine
    /// only follows it.
    Server { host: UpsHost, port: UpsServerPort, ups: UpsWord, user: UpsWord, password: UpsWord },
    /// A UPS played by NUT's `dummy-ups` from a file, for tests and for
    /// trying the settings out: its state is set with `upsrw`.
    Simulated,
}

impl UpsSource {
    /// Whether a driver on this machine speaks to the UPS; otherwise
    /// another device's does.
    pub fn is_local(&self) -> bool {
        !matches!(self, Self::Server { .. })
    }
}

/// A port of a NUT server: any but 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct UpsServerPort(u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a port from 1 to 65535")]
pub struct InvalidUpsServerPort;

impl UpsServerPort {
    pub fn new(port: u16) -> Result<Self, InvalidUpsServerPort> {
        if port == 0 { Err(InvalidUpsServerPort) } else { Ok(Self(port)) }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for UpsServerPort {
    type Error = InvalidUpsServerPort;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<UpsServerPort> for u16 {
    fn from(port: UpsServerPort) -> Self {
        port.0
    }
}

/// When the NAS shuts down while the UPS runs on battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpsShutdown {
    /// When the UPS says its battery is low (or, following another device,
    /// when that one says to shut down).
    LowBattery,
    /// When the charge falls to `percent` (5 to 95): the driver then calls
    /// the battery low itself. With a driver on this machine only.
    ChargeBelow { percent: u8 },
    /// After `minutes` (1 to 720) on battery, or earlier when the battery
    /// is low.
    AfterMinutes { minutes: u16 },
}

/// The settings NUT is given. They are checked as a whole by
/// [`UpsConfig::checked`], which the agent calls before it writes anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsConfig {
    pub source: UpsSource,
    pub shutdown: UpsShutdown,
    /// After the shutdown, the UPS is told to cut its output, so that the
    /// NAS starts again by itself once the power is back. With a driver on
    /// this machine only.
    pub cut_power: bool,
    /// Other devices may follow the UPS: NUT's server listens on the
    /// network, and they sign in with this account. With a driver on this
    /// machine only.
    pub serve: Option<UpsAccount>,
    /// The password of the monitor's own account on this machine's server.
    pub local_password: UpsWord,
}

/// The account the devices that follow this UPS sign in with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsAccount {
    pub user: UpsWord,
    pub password: UpsWord,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidUpsConfig {
    #[error("the charge to shut down at is from 5 to 95 percent")]
    Charge,
    #[error("the time on battery is from 1 to 720 minutes")]
    Minutes,
    #[error(
        "a charge to shut down at needs a UPS on this machine: one that another device has says itself when to shut down"
    )]
    ChargeNeedsDriver,
    #[error("only a UPS on this machine can be told to cut its output")]
    CutNeedsDriver,
    #[error("only a UPS on this machine can be offered to other devices")]
    ServeNeedsDriver,
    #[error("the account for other devices cannot be called {UPS_LOCAL_USER}, which the NAS uses itself")]
    AccountTaken,
}

impl UpsConfig {
    /// The settings, if they fit together.
    pub fn checked(self) -> Result<Self, InvalidUpsConfig> {
        let local = self.source.is_local();
        match self.shutdown {
            UpsShutdown::LowBattery => {}
            UpsShutdown::ChargeBelow { percent } => {
                if !(5..=95).contains(&percent) {
                    return Err(InvalidUpsConfig::Charge);
                }
                if !local {
                    return Err(InvalidUpsConfig::ChargeNeedsDriver);
                }
            }
            UpsShutdown::AfterMinutes { minutes } => {
                if !(1..=720).contains(&minutes) {
                    return Err(InvalidUpsConfig::Minutes);
                }
            }
        }
        if self.cut_power && !local {
            return Err(InvalidUpsConfig::CutNeedsDriver);
        }
        if let Some(account) = &self.serve {
            if !local {
                return Err(InvalidUpsConfig::ServeNeedsDriver);
            }
            if account.user.as_str() == UPS_LOCAL_USER {
                return Err(InvalidUpsConfig::AccountTaken);
            }
        }
        Ok(self)
    }
}

/// A UPS `nut-scanner` found on USB, as it tells it; what is chosen from
/// it is checked when it comes back as a [`UpsSource`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundUps {
    pub driver: String,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str) -> UpsWord {
        UpsWord::parse(text).unwrap()
    }

    fn config(source: UpsSource) -> UpsConfig {
        UpsConfig {
            source,
            shutdown: UpsShutdown::LowBattery,
            cut_power: false,
            serve: None,
            local_password: word("local"),
        }
    }

    fn usb() -> UpsSource {
        UpsSource::Usb { driver: UsbDriver::UsbHid, vendor_id: None, product_id: None, serial: None }
    }

    fn server() -> UpsSource {
        UpsSource::Server {
            host: UpsHost::parse("nas.local").unwrap(),
            port: UpsServerPort::new(3493).unwrap(),
            ups: word("ups"),
            user: word("monuser"),
            password: word("secret"),
        }
    }

    #[test]
    fn nothing_that_could_break_out_of_a_configuration_line_is_a_word() {
        for good in ["monuser", "s3cret!", "a.b_c-d+e:f,g/h@i%j~k^l*(m)", &"x".repeat(64)] {
            assert!(UpsWord::parse(good).is_ok(), "{good}");
        }
        for bad in
            ["", "two words", "quo\"te", "back\\slash", "hash#", "a=b", "line\nbreak", "tab\t", "ümlaut"]
        {
            assert_eq!(UpsWord::parse(bad), Err(InvalidUpsWord), "{bad:?}");
        }
        assert!(UpsWord::parse(&"x".repeat(65)).is_err());
        // Read from the wire, it is checked the same way.
        assert!(serde_json::from_str::<UpsWord>("\"fine\"").is_ok());
        assert!(serde_json::from_str::<UpsWord>("\"not fine\"").is_err());
    }

    #[test]
    fn hosts_and_usb_ids_are_what_they_say() {
        for good in ["nas", "nas.local", "192.168.1.20", "UPS-1.example.org"] {
            assert!(UpsHost::parse(good).is_ok(), "{good}");
        }
        assert_eq!(UpsHost::parse("NAS.Local").unwrap().as_str(), "nas.local");
        for bad in ["", "a b", "::1", "[::1]", "nas:3493", "-nas", "nas-", "a..b", "ups@nas"] {
            assert_eq!(UpsHost::parse(bad), Err(InvalidUpsHost), "{bad:?}");
        }
        assert_eq!(UsbId::parse("0463").unwrap().as_str(), "0463");
        assert_eq!(UsbId::parse("FFFF").unwrap().as_str(), "ffff");
        for bad in ["", "463", "04633", "046g", "0x04"] {
            assert_eq!(UsbId::parse(bad), Err(InvalidUsbId), "{bad:?}");
        }
        assert_eq!(UpsServerPort::new(0), Err(InvalidUpsServerPort));
        assert_eq!(UpsServerPort::new(3493).unwrap().get(), 3493);
    }

    #[test]
    fn every_usb_driver_has_its_name_both_ways() {
        for driver in UsbDriver::ALL {
            assert_eq!(UsbDriver::parse(driver.as_str()), Some(driver));
            assert_eq!(serde_json::to_string(&driver).unwrap(), format!("\"{}\"", driver.as_str()));
        }
        assert_eq!(UsbDriver::parse("dummy-ups"), None);
        assert_eq!(UsbDriver::parse("snmp-ups"), None);
    }

    #[test]
    fn the_settings_must_fit_the_place_of_the_ups() {
        assert!(config(usb()).checked().is_ok());
        assert!(config(server()).checked().is_ok());

        let charge = |source, percent| {
            UpsConfig { shutdown: UpsShutdown::ChargeBelow { percent }, ..config(source) }.checked()
        };
        assert!(charge(usb(), 30).is_ok());
        assert_eq!(charge(usb(), 4), Err(InvalidUpsConfig::Charge));
        assert_eq!(charge(usb(), 96), Err(InvalidUpsConfig::Charge));
        assert_eq!(charge(server(), 30), Err(InvalidUpsConfig::ChargeNeedsDriver));

        let minutes = |source, minutes| {
            UpsConfig { shutdown: UpsShutdown::AfterMinutes { minutes }, ..config(source) }.checked()
        };
        assert!(minutes(usb(), 10).is_ok());
        assert!(minutes(server(), 10).is_ok(), "a timer needs no driver");
        assert_eq!(minutes(usb(), 0), Err(InvalidUpsConfig::Minutes));
        assert_eq!(minutes(usb(), 721), Err(InvalidUpsConfig::Minutes));

        assert_eq!(
            UpsConfig { cut_power: true, ..config(server()) }.checked(),
            Err(InvalidUpsConfig::CutNeedsDriver)
        );
        let account = |user: &str| Some(UpsAccount { user: word(user), password: word("secret") });
        assert!(UpsConfig { serve: account("monuser"), ..config(usb()) }.checked().is_ok());
        assert!(UpsConfig { serve: account("monuser"), ..config(UpsSource::Simulated) }.checked().is_ok());
        assert_eq!(
            UpsConfig { serve: account("monuser"), ..config(server()) }.checked(),
            Err(InvalidUpsConfig::ServeNeedsDriver)
        );
        assert_eq!(
            UpsConfig { serve: account(UPS_LOCAL_USER), ..config(usb()) }.checked(),
            Err(InvalidUpsConfig::AccountTaken)
        );
    }

    #[test]
    fn a_source_reads_from_json_by_its_kind_and_takes_nothing_unknown() {
        let source: UpsSource = serde_json::from_str(
            r#"{"kind":"usb","driver":"usbhid-ups","vendor_id":"0463","product_id":"FFFF","serial":null}"#,
        )
        .unwrap();
        assert!(matches!(source, UpsSource::Usb { driver: UsbDriver::UsbHid, .. }));
        assert!(serde_json::from_str::<UpsSource>(r#"{"kind":"usb","driver":"dummy-ups"}"#).is_err());
        assert!(
            serde_json::from_str::<UpsSource>(
                r#"{"kind":"snmp","address":"10.0.0.9","community":"public","version":"v2c"}"#
            )
            .is_ok()
        );
        assert!(
            serde_json::from_str::<UpsSource>(
                r#"{"kind":"snmp","address":"10.0.0.9","community":"pub lic","version":"v2c"}"#
            )
            .is_err()
        );
        assert_eq!(
            serde_json::from_str::<UpsSource>(r#"{"kind":"simulated"}"#).unwrap(),
            UpsSource::Simulated
        );
        assert!(serde_json::from_str::<UpsSource>(r#"{"kind":"simulated","port":"/etc/shadow"}"#).is_err());
    }
}
