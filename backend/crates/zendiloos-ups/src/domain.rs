//! The UPS's settings and what its state means.

use std::collections::BTreeMap;

/// The name the UPS has in NUT on this machine, and the one asked for at
/// another device unless told otherwise.
pub const UPS_NAME: &str = "ups";
/// The port NUT's server listens on.
pub const UPS_PORT: u16 = 3493;
/// The account the NAS's own monitor uses, which no other may be called.
pub const LOCAL_USER: &str = "upsmon_local";
/// The account other devices sign in with unless another is chosen: the
/// name several NAS systems use without asking.
pub const DEFAULT_ACCOUNT: &str = "monuser";

/// NUT's drivers for a UPS on USB.
pub const USB_DRIVERS: [&str; 8] = [
    "usbhid-ups",
    "nutdrv_qx",
    "blazer_usb",
    "bcmxcp_usb",
    "tripplite_usb",
    "richcomm_usb",
    "riello_usb",
    "nutdrv_atcl_usb",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnmpVersion {
    V1,
    V2c,
}

/// Where the UPS is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Plugged into the NAS. The ids and the serial number name one UPS
    /// among several; without them the driver takes the first it finds.
    Usb { driver: String, vendor_id: Option<String>, product_id: Option<String>, serial: Option<String> },
    /// A UPS with a network card, asked over SNMP.
    Snmp { address: String, community: String, version: SnmpVersion },
    /// The UPS of another device that runs NUT's server; the NAS follows.
    Server { host: String, port: u16, ups: String, user: String, password: String },
    /// A UPS that NUT plays from a file, for tests.
    Simulated,
}

impl Source {
    /// Whether a driver on the NAS speaks to the UPS.
    pub fn is_local(&self) -> bool {
        !matches!(self, Self::Server { .. })
    }

    /// The source's kind, as the audit log and the store name it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Usb { .. } => "usb",
            Self::Snmp { .. } => "snmp",
            Self::Server { .. } => "server",
            Self::Simulated => "simulated",
        }
    }
}

/// When the NAS shuts down while the UPS runs on battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShutdownWhen {
    /// When the UPS says its battery is low; following another device,
    /// when that one says to shut down.
    #[default]
    LowBattery,
    /// When the charge has fallen to this many percent (5 to 95).
    ChargeBelow(u8),
    /// After this many minutes on battery (1 to 720), or earlier when the
    /// battery is low.
    AfterMinutes(u16),
}

/// The account the devices that follow this UPS sign in with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub user: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    /// None: no UPS, nothing of NUT runs.
    pub source: Option<Source>,
    pub shutdown: ShutdownWhen,
    /// After the shutdown the UPS cuts its output, so that the NAS starts
    /// again by itself when the power is back.
    pub cut_power: bool,
    /// Other devices may follow the UPS through the NAS.
    pub serve: Option<Account>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error("the driver is not one of NUT's for USB")]
    Driver,
    #[error("a USB id is four hexadecimal digits")]
    UsbId,
    #[error("the serial number may hold letters, digits and . _ - only, at most 64")]
    Serial,
    #[error("the address is a host name or an IPv4 address")]
    Host,
    #[error("the port is from 1 to 65535")]
    Port,
    #[error("the UPS's name may hold letters, digits and . _ - only, at most 64")]
    UpsName,
    #[error("the account's name may hold letters, digits and . _ - only, at most 64")]
    User,
    #[error("the password may hold 1 to 64 letters, digits and . _ - + : , / @ % ! ~ ^ * ( ), no blank")]
    Password,
    #[error("the community may hold 1 to 64 letters, digits and . _ - + : , / @ % ! ~ ^ * ( ), no blank")]
    Community,
    #[error("the charge to shut down at is from 5 to 95 percent")]
    Charge,
    #[error("the time on battery is from 1 to 720 minutes")]
    Minutes,
    #[error("a charge to shut down at needs a UPS on the NAS itself")]
    ChargeNeedsDriver,
    #[error("only a UPS on the NAS itself can be told to cut its output")]
    CutNeedsDriver,
    #[error("only a UPS on the NAS itself can be offered to other devices")]
    ServeNeedsDriver,
    #[error("that account's name is the one the NAS uses itself")]
    AccountTaken,
    #[error("without a UPS there is nothing for other devices to follow")]
    NothingToServe,
}

/// A word that can stand in one of NUT's files as it is.
fn is_word(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text.chars().all(|c| c.is_ascii_alphanumeric() || "._-+:,/@%!~^*()".contains(c))
}

/// A name: a word of letters, digits and `. _ -` only.
fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// A host name or an IPv4 address.
pub fn is_host(text: &str) -> bool {
    let label = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    !text.is_empty() && text.len() <= 253 && text.split('.').all(label)
}

fn is_usb_id(text: &str) -> bool {
    text.len() == 4 && text.chars().all(|c| c.is_ascii_hexdigit())
}

impl Settings {
    /// The settings as they are kept: checked, ids in lowercase, hosts too.
    pub fn checked(mut self) -> Result<Self, Invalid> {
        let local = self.source.as_ref().is_some_and(Source::is_local);
        match &mut self.source {
            None => {
                if self.serve.is_some() {
                    return Err(Invalid::NothingToServe);
                }
            }
            Some(Source::Usb { driver, vendor_id, product_id, serial }) => {
                if !USB_DRIVERS.contains(&driver.as_str()) {
                    return Err(Invalid::Driver);
                }
                for id in [vendor_id, product_id].into_iter().flatten() {
                    if !is_usb_id(id) {
                        return Err(Invalid::UsbId);
                    }
                    *id = id.to_ascii_lowercase();
                }
                if serial.as_deref().is_some_and(|serial| !is_name(serial)) {
                    return Err(Invalid::Serial);
                }
            }
            Some(Source::Snmp { address, community, .. }) => {
                if !is_host(address) {
                    return Err(Invalid::Host);
                }
                *address = address.to_ascii_lowercase();
                if !is_word(community) {
                    return Err(Invalid::Community);
                }
            }
            Some(Source::Server { host, port, ups, user, password }) => {
                if !is_host(host) {
                    return Err(Invalid::Host);
                }
                *host = host.to_ascii_lowercase();
                if *port == 0 {
                    return Err(Invalid::Port);
                }
                if !is_name(ups) {
                    return Err(Invalid::UpsName);
                }
                if !is_name(user) {
                    return Err(Invalid::User);
                }
                if !is_word(password) {
                    return Err(Invalid::Password);
                }
            }
            Some(Source::Simulated) => {}
        }
        match self.shutdown {
            ShutdownWhen::LowBattery => {}
            ShutdownWhen::ChargeBelow(percent) => {
                if !(5..=95).contains(&percent) {
                    return Err(Invalid::Charge);
                }
                if self.source.is_some() && !local {
                    return Err(Invalid::ChargeNeedsDriver);
                }
            }
            ShutdownWhen::AfterMinutes(minutes) => {
                if !(1..=720).contains(&minutes) {
                    return Err(Invalid::Minutes);
                }
            }
        }
        if self.cut_power && self.source.is_some() && !local {
            return Err(Invalid::CutNeedsDriver);
        }
        if let Some(account) = &self.serve {
            if !local {
                return Err(Invalid::ServeNeedsDriver);
            }
            if !is_name(&account.user) {
                return Err(Invalid::User);
            }
            if account.user == LOCAL_USER {
                return Err(Invalid::AccountTaken);
            }
            if !is_word(&account.password) {
                return Err(Invalid::Password);
            }
        }
        Ok(self)
    }
}

/// A UPS found on USB, as the scanner tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub driver: String,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
}

/// What the UPS says of itself, from NUT's variables.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpsState {
    /// On battery (`OB`): the power has failed.
    pub on_battery: bool,
    /// The battery is low (`LB`), or a shutdown is forced (`FSD`).
    pub low_battery: bool,
    /// The UPS asks for a new battery (`RB`).
    pub replace_battery: bool,
    pub charging: bool,
    /// `ups.status` as NUT gives it, e.g. `OL CHRG`.
    pub status: String,
    pub charge_percent: Option<u8>,
    pub runtime_seconds: Option<u32>,
    pub load_percent: Option<u8>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
}

impl UpsState {
    /// The state NUT's variables tell; none without `ups.status`, which a
    /// driver that has lost its UPS no longer gives.
    pub fn from_variables(variables: &BTreeMap<String, String>) -> Option<Self> {
        let status = variables.get("ups.status")?.trim().to_owned();
        let has = |flag: &str| status.split_whitespace().any(|word| word == flag);
        // NUT gives numbers as text, some with a fraction ("87.0").
        let number = |name: &str| variables.get(name)?.trim().parse::<f64>().ok().filter(|n| *n >= 0.0);
        let percent = |name: &str| number(name).map(|n| n.min(100.0).round() as u8);
        let text = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| variables.get(*name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()))
        };
        Some(Self {
            on_battery: has("OB"),
            low_battery: has("LB") || has("FSD"),
            replace_battery: has("RB"),
            charging: has("CHRG"),
            charge_percent: percent("battery.charge"),
            runtime_seconds: number("battery.runtime").map(|n| n.min(f64::from(u32::MAX)) as u32),
            load_percent: percent("ups.load"),
            manufacturer: text(&["device.mfr", "ups.mfr"]),
            model: text(&["device.model", "ups.model"]),
            status,
        })
    }

    /// Whether the UPS itself calls its battery low (`LB`), rather than a
    /// shutdown being forced for another reason (`FSD` alone: the time
    /// on battery is over).
    pub fn battery_low(&self) -> bool {
        self.status.split_whitespace().any(|word| word == "LB")
    }
}

/// What a change of the UPS's state is worth telling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The power has failed: the UPS runs on battery.
    OnBattery,
    /// The power is back.
    OnMains,
    /// The battery is low, or a shutdown is forced: the NAS shuts down.
    ShuttingDown,
    /// The UPS has not answered for a while.
    Unreachable,
    /// The UPS asks for a new battery.
    ReplaceBattery,
}

/// How many readings in a row may fail before the UPS counts as not
/// answering: a driver that restarts, a cable pushed back in.
pub const MISSES_BEFORE_UNREACHABLE: u8 = 3;

/// The UPS as it was last seen, to tell what changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watch {
    seen: Option<Seen>,
    misses: u8,
    unreachable_told: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    on_battery: bool,
    low_battery: bool,
    replace_battery: bool,
}

impl Watch {
    /// Takes a reading (none: the UPS did not answer) and says what to
    /// tell. The first reading tells nothing but trouble that is there
    /// already: a NAS that starts on battery says so.
    pub fn observe(&mut self, state: Option<&UpsState>) -> Vec<Event> {
        let Some(state) = state else {
            self.misses = self.misses.saturating_add(1);
            if self.misses >= MISSES_BEFORE_UNREACHABLE && !self.unreachable_told {
                self.unreachable_told = true;
                return vec![Event::Unreachable];
            }
            return Vec::new();
        };
        self.misses = 0;
        self.unreachable_told = false;
        let now = Seen {
            on_battery: state.on_battery,
            low_battery: state.low_battery,
            replace_battery: state.replace_battery,
        };
        let before = self.seen.replace(now).unwrap_or(Seen {
            on_battery: false,
            low_battery: false,
            replace_battery: false,
        });
        let mut events = Vec::new();
        if now.on_battery && !before.on_battery {
            events.push(Event::OnBattery);
        }
        if !now.on_battery && before.on_battery {
            events.push(Event::OnMains);
        }
        if now.low_battery && !before.low_battery {
            events.push(Event::ShuttingDown);
        }
        if now.replace_battery && !before.replace_battery {
            events.push(Event::ReplaceBattery);
        }
        events
    }

    /// Nothing is watched any more: the next UPS starts afresh.
    pub fn forget(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb() -> Source {
        Source::Usb { driver: "usbhid-ups".into(), vendor_id: None, product_id: None, serial: None }
    }

    fn server() -> Source {
        Source::Server {
            host: "NAS.local".into(),
            port: 3493,
            ups: "ups".into(),
            user: "monuser".into(),
            password: "secret".into(),
        }
    }

    fn with(source: Source) -> Settings {
        Settings { source: Some(source), ..Settings::default() }
    }

    #[test]
    fn no_ups_is_a_setting_too_but_nothing_to_serve() {
        assert_eq!(Settings::default().checked(), Ok(Settings::default()));
        let serve = Settings {
            serve: Some(Account { user: "monuser".into(), password: "secret".into() }),
            ..Settings::default()
        };
        assert_eq!(serve.checked(), Err(Invalid::NothingToServe));
    }

    #[test]
    fn a_usb_ups_names_a_driver_of_nuts_and_ids_in_hex() {
        assert!(with(usb()).checked().is_ok());
        let named = Source::Usb {
            driver: "usbhid-ups".into(),
            vendor_id: Some("0463".into()),
            product_id: Some("FFFF".into()),
            serial: Some("P123-456".into()),
        };
        let kept = with(named).checked().unwrap();
        assert!(matches!(&kept.source, Some(Source::Usb { product_id: Some(id), .. }) if id == "ffff"));
        let usb = |driver: &str, vendor: Option<&str>, serial: Option<&str>| {
            with(Source::Usb {
                driver: driver.into(),
                vendor_id: vendor.map(str::to_owned),
                product_id: None,
                serial: serial.map(str::to_owned),
            })
            .checked()
        };
        assert_eq!(usb("dummy-ups", None, None), Err(Invalid::Driver));
        assert_eq!(usb("usbhid-ups; rm", None, None), Err(Invalid::Driver));
        assert_eq!(usb("usbhid-ups", Some("46g3"), None), Err(Invalid::UsbId));
        assert_eq!(usb("usbhid-ups", None, Some("two words")), Err(Invalid::Serial));
    }

    #[test]
    fn what_reaches_nuts_files_holds_no_blank_quote_or_comment_sign() {
        let snmp = |address: &str, community: &str| {
            with(Source::Snmp {
                address: address.into(),
                community: community.into(),
                version: SnmpVersion::V2c,
            })
            .checked()
        };
        assert!(snmp("10.0.0.9", "public").is_ok());
        assert_eq!(snmp("10.0.0.9 x", "public"), Err(Invalid::Host));
        assert_eq!(snmp("::1", "public"), Err(Invalid::Host));
        assert_eq!(snmp("10.0.0.9", "pub lic"), Err(Invalid::Community));
        assert_eq!(snmp("10.0.0.9", "pub\"lic"), Err(Invalid::Community));
        assert_eq!(snmp("10.0.0.9", "pub#lic"), Err(Invalid::Community));
        assert_eq!(snmp("10.0.0.9", "line\nbreak"), Err(Invalid::Community));

        let kept = with(server()).checked().unwrap();
        assert!(matches!(&kept.source, Some(Source::Server { host, .. }) if host == "nas.local"));
        let server = |change: fn(&mut Source)| {
            let mut source = server();
            change(&mut source);
            with(source).checked()
        };
        assert_eq!(
            server(|s| if let Source::Server { port, .. } = s {
                *port = 0
            }),
            Err(Invalid::Port)
        );
        assert_eq!(
            server(|s| if let Source::Server { ups, .. } = s {
                *ups = "ups@host".into()
            }),
            Err(Invalid::UpsName)
        );
        assert_eq!(
            server(|s| if let Source::Server { user, .. } = s {
                *user = String::new()
            }),
            Err(Invalid::User)
        );
        assert_eq!(
            server(|s| if let Source::Server { password, .. } = s {
                *password = "two words".into()
            }),
            Err(Invalid::Password)
        );
    }

    #[test]
    fn the_shutdown_and_the_extras_must_fit_where_the_ups_is() {
        let shutdown = |source, when| Settings { shutdown: when, ..with(source) }.checked();
        assert!(shutdown(usb(), ShutdownWhen::ChargeBelow(30)).is_ok());
        assert_eq!(shutdown(usb(), ShutdownWhen::ChargeBelow(4)), Err(Invalid::Charge));
        assert_eq!(shutdown(usb(), ShutdownWhen::ChargeBelow(96)), Err(Invalid::Charge));
        assert_eq!(shutdown(server(), ShutdownWhen::ChargeBelow(30)), Err(Invalid::ChargeNeedsDriver));
        assert!(shutdown(usb(), ShutdownWhen::AfterMinutes(10)).is_ok());
        assert!(shutdown(server(), ShutdownWhen::AfterMinutes(10)).is_ok());
        assert_eq!(shutdown(usb(), ShutdownWhen::AfterMinutes(0)), Err(Invalid::Minutes));
        assert_eq!(shutdown(usb(), ShutdownWhen::AfterMinutes(721)), Err(Invalid::Minutes));

        assert!(Settings { cut_power: true, ..with(usb()) }.checked().is_ok());
        assert_eq!(Settings { cut_power: true, ..with(server()) }.checked(), Err(Invalid::CutNeedsDriver));

        let serve = |source, user: &str, password: &str| {
            Settings { serve: Some(Account { user: user.into(), password: password.into() }), ..with(source) }
                .checked()
        };
        assert!(serve(usb(), "monuser", "secret").is_ok());
        assert!(serve(Source::Simulated, "monuser", "secret").is_ok());
        assert_eq!(serve(server(), "monuser", "secret"), Err(Invalid::ServeNeedsDriver));
        assert_eq!(serve(usb(), LOCAL_USER, "secret"), Err(Invalid::AccountTaken));
        assert_eq!(serve(usb(), "mon user", "secret"), Err(Invalid::User));
        assert_eq!(serve(usb(), "monuser", ""), Err(Invalid::Password));
    }

    fn variables(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn nuts_variables_become_a_state() {
        let state = UpsState::from_variables(&variables(&[
            ("ups.status", "OL CHRG"),
            ("battery.charge", "87.0"),
            ("battery.runtime", "1432"),
            ("ups.load", "23"),
            ("device.mfr", "EATON"),
            ("ups.model", "Ellipse ECO 650"),
        ]))
        .unwrap();
        assert_eq!(
            state,
            UpsState {
                on_battery: false,
                low_battery: false,
                replace_battery: false,
                charging: true,
                status: "OL CHRG".into(),
                charge_percent: Some(87),
                runtime_seconds: Some(1432),
                load_percent: Some(23),
                manufacturer: Some("EATON".into()),
                model: Some("Ellipse ECO 650".into()),
            }
        );
        let low = UpsState::from_variables(&variables(&[("ups.status", "OB DISCHRG LB RB")])).unwrap();
        assert!(low.on_battery && low.low_battery && low.replace_battery && !low.charging);
        assert_eq!((low.charge_percent, low.runtime_seconds, low.model.as_deref()), (None, None, None));
        // A forced shutdown counts as a low battery: the NAS goes down.
        assert!(UpsState::from_variables(&variables(&[("ups.status", "OL FSD")])).unwrap().low_battery);
        // What is no number, or too large, does not become one.
        let odd = UpsState::from_variables(&variables(&[
            ("ups.status", "OL"),
            ("battery.charge", "full"),
            ("ups.load", "250"),
            ("battery.runtime", "-5"),
        ]))
        .unwrap();
        assert_eq!((odd.charge_percent, odd.load_percent, odd.runtime_seconds), (None, Some(100), None));
        // A driver that lost its UPS gives no status.
        assert_eq!(UpsState::from_variables(&variables(&[("driver.name", "usbhid-ups")])), None);
    }

    fn state(status: &str) -> UpsState {
        UpsState::from_variables(&variables(&[("ups.status", status)])).unwrap()
    }

    #[test]
    fn a_power_cut_is_told_once_and_so_is_its_end() {
        let mut watch = Watch::default();
        assert_eq!(watch.observe(Some(&state("OL"))), [], "all is well at the start");
        assert_eq!(watch.observe(Some(&state("OL CHRG"))), []);
        assert_eq!(watch.observe(Some(&state("OB DISCHRG"))), [Event::OnBattery]);
        assert_eq!(watch.observe(Some(&state("OB DISCHRG"))), [], "not again");
        assert_eq!(watch.observe(Some(&state("OB DISCHRG LB"))), [Event::ShuttingDown]);
        assert!(state("OB DISCHRG LB").battery_low());
        // A shutdown forced after a time on battery: not the battery's doing.
        assert!(state("OB FSD").low_battery && !state("OB FSD").battery_low());
        assert_eq!(watch.observe(Some(&state("OL CHRG"))), [Event::OnMains]);
        assert_eq!(watch.observe(Some(&state("OL RB"))), [Event::ReplaceBattery]);
        assert_eq!(watch.observe(Some(&state("OL RB"))), []);
    }

    #[test]
    fn a_nas_that_starts_on_battery_says_so() {
        let mut watch = Watch::default();
        assert_eq!(watch.observe(Some(&state("OB LB"))), [Event::OnBattery, Event::ShuttingDown]);
    }

    #[test]
    fn a_ups_that_stops_answering_is_told_after_a_few_misses_and_once() {
        let mut watch = Watch::default();
        watch.observe(Some(&state("OL")));
        assert_eq!(watch.observe(None), []);
        assert_eq!(watch.observe(None), []);
        assert_eq!(watch.observe(None), [Event::Unreachable]);
        assert_eq!(watch.observe(None), [], "once");
        // Back, it is watched as before, and a new silence is told again.
        assert_eq!(watch.observe(Some(&state("OL"))), []);
        for _ in 0..2 {
            assert_eq!(watch.observe(None), []);
        }
        assert_eq!(watch.observe(None), [Event::Unreachable]);
        // What happened while it was silent shows when it answers.
        assert_eq!(watch.observe(Some(&state("OB"))), [Event::OnBattery]);
        watch.forget();
        assert_eq!(watch, Watch::default());
    }
}
