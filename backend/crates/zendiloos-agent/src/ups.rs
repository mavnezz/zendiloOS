//! A UPS through Network UPS Tools (NUT) from Debian: the agent writes
//! NUT's configuration and starts what the settings need; NUT does the
//! rest, also while zendiloosd is down.
//!
//! - A driver on this machine (USB, SNMP, or `dummy-ups` for tests) is
//!   always called `ups`. Debian's `nut-driver-enumerator` makes the unit
//!   `nut-driver@ups` from `ups.conf`; `upsd` serves it, on this machine
//!   only or, for other devices, on the network.
//! - `upsmon` watches the UPS (this machine's or another device's) and
//!   shuts the machine down with `shutdown -h +0` when the battery is low.
//!   A charge to shut down at is the driver's doing (`ignorelb` and
//!   `override.battery.charge.low`); a time on battery is a timer of
//!   `upssched` that asks `upsmon` for a forced shutdown, which the devices
//!   that follow this one are told first.
//! - With `POWERDOWNFLAG`, Debian's shutdown hook (`nutshutdown`) tells the
//!   UPS to cut its output once the machine is down, so it starts again
//!   when the power returns.
//!
//! The files are NUT's own, replaced whole, readable by root and the group
//! of `/etc/nut` (Debian's `nut`) as the package has them: they hold the
//! accounts' passwords. What stood in one before zendiloOS wrote it the
//! first time (Debian's sample, or a configuration made by hand) is kept
//! beside it, as `<name>.before-zendiloos`. A driver that does not start
//! (the UPS unplugged) does not make the settings fail: the status then
//! says that the UPS does not answer.
//!
//! Settings that are in force already change nothing and interrupt
//! nothing: zendiloosd asks for them again at every start, so that the
//! machine follows a restored configuration. Without a UPS, NUT is
//! switched off only where zendiloOS set it up: one configured by hand and
//! never through zendiloOS is left as it is.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::Duration;

use zendiloos_agent_proto::{
    FoundUps, Response, UPS_LOCAL_USER, UPS_NAME, UPS_PORT, UpsConfig, UpsShutdown, UpsSource,
};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;
use crate::pools::{link_units, reload_if_behind};

const LIMITS: Limits = Limits { timeout: Duration::from_secs(60), max_stdout: 64 * 1024 };
const SERVER: &str = "nut-server.service";
const MONITOR: &str = "nut-monitor.service";
const ENUMERATOR: &str = "nut-driver-enumerator.service";
const DRIVER: &str = "nut-driver@ups.service";
const HEADER: &str = "# Written by zendiloOS; changes made here are replaced.\n";
/// The file NUT's `dummy-ups` plays a UPS from, in NUT's folder.
const SIMULATED: &str = "zendiloos-simulated.dev";
/// What `upssched` runs when its timer ends, in NUT's folder.
const TIMER: &str = "zendiloos-timer";
/// The timer's name in `upssched.conf`.
const ON_BATTERY: &str = "onbatt";
/// What a file's copy from before zendiloOS wrote it is called, after its name.
const BEFORE: &str = ".before-zendiloos";

/// Where NUT's files are; other than the real places in tests.
#[derive(Debug, Clone)]
pub struct UpsPaths {
    /// NUT's configuration, `/etc/nut`.
    pub dir: PathBuf,
    /// The SNMP driver, which comes with `nut-snmp`.
    pub snmp_driver: PathBuf,
}

impl Default for UpsPaths {
    fn default() -> Self {
        Self { dir: PathBuf::from("/etc/nut"), snmp_driver: PathBuf::from("/lib/nut/snmp-ups") }
    }
}

/// What writing NUT's files changed.
#[derive(Debug, Clone, Copy)]
struct Written {
    /// One of the files.
    any: bool,
    /// `ups.conf`, which the driver reads when it starts.
    driver: bool,
}

#[derive(Debug)]
pub(crate) struct Ups {
    systemctl: Program,
    systemd_run: Program,
    upsd: Program,
    upsmon: Program,
    scanner: Program,
    paths: UpsPaths,
}

/// NUT's mode for `nut.conf`.
fn mode(config: Option<&UpsConfig>) -> &'static str {
    match config {
        None => "none",
        Some(config) if !config.source.is_local() => "netclient",
        Some(config) if config.serve.is_some() => "netserver",
        Some(_) => "standalone",
    }
}

pub fn nut_conf(config: Option<&UpsConfig>) -> String {
    format!("{HEADER}MODE={}\n", mode(config))
}

/// The driver on this machine, or nothing for a UPS another device has.
pub fn ups_conf(config: Option<&UpsConfig>) -> String {
    let mut text = format!("{HEADER}maxretry = 3\n");
    let Some(config) = config.filter(|config| config.source.is_local()) else {
        return text;
    };
    text.push_str(&format!("\n[{UPS_NAME}]\n"));
    let mut line = |key: &str, value: &str| text.push_str(&format!("    {key} = {value}\n"));
    match &config.source {
        UpsSource::Usb { driver, vendor_id, product_id, serial } => {
            line("driver", driver.as_str());
            line("port", "auto");
            if let Some(id) = vendor_id {
                line("vendorid", id.as_str());
            }
            if let Some(id) = product_id {
                line("productid", id.as_str());
            }
            if let Some(serial) = serial {
                line("serial", serial.as_str());
            }
        }
        UpsSource::Snmp { address, community, version } => {
            line("driver", "snmp-ups");
            line("port", address.as_str());
            line("community", community.as_str());
            line("snmp_version", version.as_str());
            line("pollfreq", "15");
        }
        UpsSource::Simulated => {
            line("driver", "dummy-ups");
            line("port", SIMULATED);
        }
        UpsSource::Server { .. } => {}
    }
    if let UpsShutdown::ChargeBelow { percent } = config.shutdown {
        // The driver then calls the battery low at this charge, whatever
        // the UPS itself thinks low.
        text.push_str("    ignorelb\n");
        text.push_str(&format!("    override.battery.charge.low = {percent}\n"));
    }
    text
}

/// On this machine only, or on the network for the devices that follow.
pub fn upsd_conf(config: Option<&UpsConfig>) -> String {
    let everyone = config.is_some_and(|config| config.serve.is_some());
    format!("{HEADER}LISTEN {} {UPS_PORT}\n", if everyone { "0.0.0.0" } else { "127.0.0.1" })
}

pub fn upsd_users(config: Option<&UpsConfig>) -> String {
    let mut text = HEADER.to_owned();
    let Some(config) = config.filter(|config| config.source.is_local()) else {
        return text;
    };
    text.push_str(&format!(
        "\n[{UPS_LOCAL_USER}]\n    password = {}\n    upsmon primary\n",
        config.local_password.as_str()
    ));
    if config.source == UpsSource::Simulated {
        // What plays a power cut: `upsrw -s ups.status=OB`.
        text.push_str("    actions = SET\n    instcmds = ALL\n");
    }
    if let Some(account) = &config.serve {
        text.push_str(&format!(
            "\n[{}]\n    password = {}\n    upsmon secondary\n",
            account.user.as_str(),
            account.password.as_str()
        ));
    }
    text
}

pub fn upsmon_conf(config: Option<&UpsConfig>) -> String {
    let mut text = HEADER.to_owned();
    let Some(config) = config else {
        return text;
    };
    match &config.source {
        UpsSource::Server { host, port, ups, user, password } => text.push_str(&format!(
            "MONITOR {}@{}:{} 1 {} {} secondary\n",
            ups.as_str(),
            host.as_str(),
            port.get(),
            user.as_str(),
            password.as_str()
        )),
        _ => text.push_str(&format!(
            "MONITOR {UPS_NAME}@localhost 1 {UPS_LOCAL_USER} {} primary\n",
            config.local_password.as_str()
        )),
    }
    text.push_str(
        "MINSUPPLIES 1\n\
         SHUTDOWNCMD \"/sbin/shutdown -h +0\"\n\
         POLLFREQ 5\n\
         POLLFREQALERT 5\n\
         HOSTSYNC 15\n\
         DEADTIME 15\n\
         FINALDELAY 10\n",
    );
    if config.cut_power {
        text.push_str("POWERDOWNFLAG /etc/killpower\n");
    }
    if matches!(config.shutdown, UpsShutdown::AfterMinutes { .. }) {
        text.push_str(
            "NOTIFYCMD /usr/sbin/upssched\n\
             NOTIFYFLAG ONBATT SYSLOG+EXEC\n\
             NOTIFYFLAG ONLINE SYSLOG+EXEC\n",
        );
    }
    text
}

/// The timer for a time on battery: started when the power fails, dropped
/// when it returns.
pub fn upssched_conf(config: Option<&UpsConfig>, dir: &std::path::Path) -> String {
    let mut text = HEADER.to_owned();
    if let Some(UpsConfig { shutdown: UpsShutdown::AfterMinutes { minutes }, .. }) = config {
        text.push_str(&format!(
            "CMDSCRIPT {}\n\
             PIPEFN /run/nut/upssched.pipe\n\
             LOCKFN /run/nut/upssched.lock\n\
             AT ONBATT * START-TIMER {ON_BATTERY} {}\n\
             AT ONLINE * CANCEL-TIMER {ON_BATTERY}\n",
            dir.join(TIMER).display(),
            u32::from(*minutes) * 60
        ));
    }
    text
}

/// What the timer runs: a forced shutdown, which `upsmon` tells the devices
/// that follow before this machine goes down.
fn timer_script() -> String {
    format!("#!/bin/sh\n{HEADER}[ \"$1\" = {ON_BATTERY} ] && exec /usr/sbin/upsmon -c fsd\nexit 0\n")
}

/// A UPS as `dummy-ups` first shows it: on mains, full.
const SIMULATED_UPS: &str = "ups.status: OL\n\
                             battery.charge: 100\n\
                             battery.runtime: 1800\n\
                             ups.load: 20\n\
                             ups.mfr: zendiloOS\n\
                             ups.model: Simulated UPS\n";

/// What `nut-scanner -U -q` prints: a section per device, `key = "value"`.
pub fn found(listing: &str) -> Vec<FoundUps> {
    let mut devices: Vec<FoundUps> = Vec::new();
    for line in listing.lines().map(str::trim) {
        if line.starts_with('[') {
            devices.push(FoundUps {
                driver: String::new(),
                vendor_id: None,
                product_id: None,
                vendor: None,
                product: None,
                serial: None,
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let Some(device) = devices.last_mut() else { continue };
        let value = value.trim().trim_matches('"').trim().to_owned();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "driver" => device.driver = value,
            "vendorid" => device.vendor_id = Some(value),
            "productid" => device.product_id = Some(value),
            "vendor" => device.vendor = Some(value),
            "product" => device.product = Some(value),
            "serial" => device.serial = Some(value),
            _ => {}
        }
    }
    devices.retain(|device| !device.driver.is_empty());
    devices
}

impl Ups {
    pub(crate) fn new(tools: &Tools, paths: UpsPaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            systemctl: Program::new(&tools.systemctl)?,
            systemd_run: Program::new(&tools.systemd_run)?,
            upsd: Program::new(&tools.upsd)?,
            upsmon: Program::new(&tools.upsmon)?,
            scanner: Program::new(&tools.nut_scanner)?,
            paths,
        })
    }

    fn installed(&self) -> bool {
        self.upsd.path().exists() && self.upsmon.path().exists()
    }

    pub(crate) fn facts(&self) -> Response {
        Response::UpsFacts { installed: self.installed(), snmp: self.paths.snmp_driver.exists() }
    }

    /// `nut-scanner` in a unit of its own: it opens the USB devices, which
    /// the agent's own unit keeps away from it.
    pub(crate) async fn scan(&self) -> Result<Response, OperationError> {
        if !self.scanner.path().exists() {
            return Err(not_installed());
        }
        let scanner = self.scanner.path().to_string_lossy().into_owned();
        let output = zendiloos_exec::run(
            &self.systemd_run,
            &["--wait", "--pipe", "--collect", "--quiet", &scanner, "-U", "-q"],
            LIMITS,
        )
        .await?;
        // Nothing found is an answer, not a failure, whatever it exits with.
        let listing = String::from_utf8_lossy(&output.stdout);
        Ok(Response::UpsFound { devices: found(&listing) })
    }

    pub(crate) async fn set(&self, config: Option<&UpsConfig>) -> Result<Response, OperationError> {
        let Some(config) = config else {
            return self.off().await;
        };
        let config = &config.clone().checked().map_err(|error| OperationError::Refused(error.to_string()))?;
        if !self.installed() {
            return Err(not_installed());
        }
        if matches!(config.source, UpsSource::Snmp { .. }) && !self.paths.snmp_driver.exists() {
            return Err(OperationError::Setup("NUT's SNMP driver is not installed (nut-snmp)".into()));
        }
        let local = config.source.is_local();
        let written = self.write_all(Some(config))?;
        // In force already, as when zendiloosd asks again at its start:
        // NUT is not interrupted for it.
        if !written.any && self.all_active(if local { &[SERVER, MONITOR] } else { &[MONITOR] }).await {
            return Ok(Response::Done);
        }
        // The driver's unit follows `ups.conf`: Debian's enumerator makes
        // it for a UPS on this machine and removes it for another
        // device's. Neither the enumerator, which can wait a minute and a
        // half for a driver, nor the driver is waited for: a UPS that is
        // not plugged in, or does not answer on the network, keeps its
        // driver from starting for as long as it tries, and systemd goes
        // on trying. The settings hold all the same, and the server takes
        // the driver once it is there.
        self.run(&["restart", "--no-block", ENUMERATOR]).await?;
        if local {
            link_units(&self.systemctl, &["enable", "--no-reload", SERVER, MONITOR]).await?;
            reload_if_behind(&self.systemctl, SERVER).await?;
            // A driver whose section changed is started anew here: the
            // enumerator of NUT 2.8.1 settles for the driver's "live
            // reload", which says it sufficed and leaves `ignorelb` as it
            // was, so that a charge to shut down at would not be in force.
            // One whose section stayed is only started, should someone
            // have stopped it.
            let how = if written.driver { "restart" } else { "start" };
            let _ = self.run(&[how, "--no-block", DRIVER]).await;
            self.run(&["restart", SERVER]).await?;
        } else {
            link_units(&self.systemctl, &["disable", "--no-reload", "--now", SERVER]).await?;
            link_units(&self.systemctl, &["enable", "--no-reload", MONITOR]).await?;
            reload_if_behind(&self.systemctl, MONITOR).await?;
        }
        self.run(&["restart", MONITOR]).await?;
        Ok(Response::Done)
    }

    /// No UPS: nothing of NUT runs, and its files say so. A NUT that is
    /// not installed, or was set up by hand and never through zendiloOS, is
    /// not zendiloOS's to switch off.
    async fn off(&self) -> Result<Response, OperationError> {
        if !self.installed() || !self.ours() {
            return Ok(Response::Done);
        }
        let written = self.write_all(None)?;
        if !written.any && !self.any_active(&[MONITOR, SERVER]).await {
            return Ok(Response::Done);
        }
        link_units(&self.systemctl, &["disable", "--no-reload", "--now", MONITOR, SERVER]).await?;
        reload_if_behind(&self.systemctl, SERVER).await?;
        // Without a section in `ups.conf` the driver's unit goes.
        let _ = self.run(&["restart", "--no-block", ENUMERATOR]).await;
        let _ = self.run(&["stop", "--no-block", DRIVER]).await;
        Ok(Response::Done)
    }

    /// Whether zendiloOS wrote NUT's configuration: `nut.conf` says so.
    fn ours(&self) -> bool {
        fs::read_to_string(self.paths.dir.join("nut.conf")).is_ok_and(|text| text.contains(HEADER))
    }

    /// NUT's files for `config`, and what of them changed.
    fn write_all(&self, config: Option<&UpsConfig>) -> Result<Written, OperationError> {
        let dir = &self.paths.dir;
        let driver = self.write("ups.conf", &ups_conf(config), 0o640)?;
        let mut changed = driver | self.write("nut.conf", &nut_conf(config), 0o640)?;
        changed |= self.write("upsd.conf", &upsd_conf(config), 0o640)?;
        changed |= self.write("upsd.users", &upsd_users(config), 0o640)?;
        changed |= self.write("upsmon.conf", &upsmon_conf(config), 0o640)?;
        changed |= self.write("upssched.conf", &upssched_conf(config, dir), 0o640)?;
        changed |= self.write(TIMER, &timer_script(), 0o750)?;
        let simulated = dir.join(SIMULATED);
        if config.is_some_and(|config| config.source == UpsSource::Simulated) {
            // Kept as it is once there: a test may have written its own.
            if !simulated.exists() {
                changed |= self.write(SIMULATED, SIMULATED_UPS, 0o640)?;
            }
        } else {
            changed |= fs::remove_file(&simulated).is_ok();
        }
        Ok(Written { any: changed, driver })
    }

    /// A file in NUT's folder, with the folder's group, which NUT's
    /// programs run in; whether it changed. What stood there before
    /// zendiloOS wrote it the first time is kept beside it.
    fn write(&self, name: &str, content: &str, mode: u32) -> Result<bool, OperationError> {
        let dir = &self.paths.dir;
        let path = dir.join(name);
        let before = fs::read_to_string(&path).ok();
        if before.as_deref() == Some(content) {
            return Ok(false);
        }
        let group = fs::metadata(dir).map_err(|e| OperationError::Read(dir.clone(), e))?.gid();
        let place = |path: PathBuf, content: &str, mode: u32| {
            crate::durable::replace_mode(&path, content.as_bytes(), mode)?;
            std::os::unix::fs::chown(&path, None, Some(group)).map_err(|e| OperationError::Write(path, e))
        };
        if let Some(before) = before.filter(|text| !text.contains(HEADER)) {
            let kept = dir.join(format!("{name}{BEFORE}"));
            if !kept.exists() {
                place(kept, &before, 0o640)?;
            }
        }
        place(path, content, mode)?;
        Ok(true)
    }

    /// Whether every one of `units` runs.
    async fn all_active(&self, units: &[&str]) -> bool {
        for unit in units {
            if !self.any_active(&[unit]).await {
                return false;
            }
        }
        true
    }

    /// Whether one of `units` runs, as `systemctl is-active` tells several.
    async fn any_active(&self, units: &[&str]) -> bool {
        let mut args = vec!["is-active", "--quiet"];
        args.extend_from_slice(units);
        zendiloos_exec::run(&self.systemctl, &args, LIMITS).await.is_ok_and(|output| output.status.success())
    }

    async fn run(&self, args: &[&str]) -> Result<(), OperationError> {
        zendiloos_exec::run(&self.systemctl, args, LIMITS).await?.success(&self.systemctl)?;
        Ok(())
    }
}

fn not_installed() -> OperationError {
    OperationError::Setup("Network UPS Tools are not installed".into())
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{SnmpVersion, UpsAccount, UpsHost, UpsServerPort, UpsWord, UsbDriver, UsbId};

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
            local_password: word("local-pw"),
        }
    }

    fn usb() -> UpsSource {
        UpsSource::Usb {
            driver: UsbDriver::UsbHid,
            vendor_id: Some(UsbId::parse("0463").unwrap()),
            product_id: Some(UsbId::parse("FFFF").unwrap()),
            serial: None,
        }
    }

    fn server() -> UpsSource {
        UpsSource::Server {
            host: UpsHost::parse("192.168.1.9").unwrap(),
            port: UpsServerPort::new(3493).unwrap(),
            ups: word("ups"),
            user: word("monuser"),
            password: word("secret"),
        }
    }

    const DIR: &str = "/etc/nut";

    fn dir() -> &'static std::path::Path {
        std::path::Path::new(DIR)
    }

    #[test]
    fn a_usb_ups_gets_its_driver_and_a_server_for_this_machine_only() {
        let config = config(usb());
        assert_eq!(nut_conf(Some(&config)), format!("{HEADER}MODE=standalone\n"));
        assert_eq!(
            ups_conf(Some(&config)),
            format!(
                "{HEADER}maxretry = 3\n\n[ups]\n    driver = usbhid-ups\n    port = auto\n    vendorid = 0463\n    productid = ffff\n"
            )
        );
        assert_eq!(upsd_conf(Some(&config)), format!("{HEADER}LISTEN 127.0.0.1 3493\n"));
        assert_eq!(
            upsd_users(Some(&config)),
            format!("{HEADER}\n[upsmon_local]\n    password = local-pw\n    upsmon primary\n")
        );
        let monitor = upsmon_conf(Some(&config));
        assert!(monitor.contains("MONITOR ups@localhost 1 upsmon_local local-pw primary\n"), "{monitor}");
        assert!(monitor.contains("SHUTDOWNCMD \"/sbin/shutdown -h +0\"\n"));
        assert!(!monitor.contains("POWERDOWNFLAG"), "the UPS keeps its output");
        assert!(!monitor.contains("NOTIFYCMD"), "no timer");
        assert_eq!(upssched_conf(Some(&config), dir()), HEADER);
    }

    #[test]
    fn a_charge_to_shut_down_at_is_the_drivers_low_battery() {
        let config = UpsConfig { shutdown: UpsShutdown::ChargeBelow { percent: 30 }, ..config(usb()) };
        let ups = ups_conf(Some(&config));
        assert!(ups.ends_with("    ignorelb\n    override.battery.charge.low = 30\n"), "{ups}");
    }

    #[test]
    fn a_time_on_battery_is_a_timer_that_forces_the_shutdown() {
        let config = UpsConfig { shutdown: UpsShutdown::AfterMinutes { minutes: 10 }, ..config(usb()) };
        let monitor = upsmon_conf(Some(&config));
        assert!(monitor.contains("NOTIFYCMD /usr/sbin/upssched\n"));
        assert!(monitor.contains("NOTIFYFLAG ONBATT SYSLOG+EXEC\n"));
        assert!(monitor.contains("NOTIFYFLAG ONLINE SYSLOG+EXEC\n"));
        assert_eq!(
            upssched_conf(Some(&config), dir()),
            format!(
                "{HEADER}CMDSCRIPT /etc/nut/zendiloos-timer\nPIPEFN /run/nut/upssched.pipe\nLOCKFN /run/nut/upssched.lock\nAT ONBATT * START-TIMER onbatt 600\nAT ONLINE * CANCEL-TIMER onbatt\n"
            )
        );
        assert!(timer_script().contains("[ \"$1\" = onbatt ] && exec /usr/sbin/upsmon -c fsd\n"));
    }

    #[test]
    fn other_devices_follow_through_a_server_on_the_network_and_an_account() {
        let config = UpsConfig {
            cut_power: true,
            serve: Some(UpsAccount { user: word("monuser"), password: word("secret") }),
            ..config(usb())
        };
        assert_eq!(nut_conf(Some(&config)), format!("{HEADER}MODE=netserver\n"));
        assert_eq!(upsd_conf(Some(&config)), format!("{HEADER}LISTEN 0.0.0.0 3493\n"));
        let users = upsd_users(Some(&config));
        assert!(users.ends_with("\n[monuser]\n    password = secret\n    upsmon secondary\n"), "{users}");
        assert!(users.contains("[upsmon_local]"));
        assert!(upsmon_conf(Some(&config)).contains("POWERDOWNFLAG /etc/killpower\n"));
    }

    #[test]
    fn another_devices_ups_needs_only_the_monitor() {
        let config = config(server());
        assert_eq!(nut_conf(Some(&config)), format!("{HEADER}MODE=netclient\n"));
        assert_eq!(ups_conf(Some(&config)), format!("{HEADER}maxretry = 3\n"), "no driver");
        assert_eq!(upsd_users(Some(&config)), HEADER, "no account: no server");
        let monitor = upsmon_conf(Some(&config));
        assert!(monitor.contains("MONITOR ups@192.168.1.9:3493 1 monuser secret secondary\n"), "{monitor}");
    }

    #[test]
    fn snmp_and_the_simulated_ups_have_their_drivers() {
        let snmp = config(UpsSource::Snmp {
            address: UpsHost::parse("10.0.0.9").unwrap(),
            community: word("public"),
            version: SnmpVersion::V2c,
        });
        let ups = ups_conf(Some(&snmp));
        assert!(
            ups.ends_with(
                "[ups]\n    driver = snmp-ups\n    port = 10.0.0.9\n    community = public\n    snmp_version = v2c\n    pollfreq = 15\n"
            ),
            "{ups}"
        );
        let simulated = config(UpsSource::Simulated);
        assert!(
            ups_conf(Some(&simulated))
                .ends_with("[ups]\n    driver = dummy-ups\n    port = zendiloos-simulated.dev\n")
        );
        // Its state is set with upsrw, which the monitor's account may then do.
        assert!(upsd_users(Some(&simulated)).contains("    actions = SET\n    instcmds = ALL\n"));
        assert!(!upsd_users(Some(&config(usb()))).contains("actions"));
    }

    #[test]
    fn without_a_ups_the_files_say_that_nothing_runs() {
        assert_eq!(nut_conf(None), format!("{HEADER}MODE=none\n"));
        assert_eq!(ups_conf(None), format!("{HEADER}maxretry = 3\n"));
        assert_eq!(upsd_users(None), HEADER);
        assert_eq!(upsmon_conf(None), HEADER);
        assert_eq!(upssched_conf(None, dir()), HEADER);
    }

    #[test]
    fn the_scanners_listing_becomes_devices() {
        let listing = "[nutdev1]\n\tdriver = \"usbhid-ups\"\n\tport = \"auto\"\n\tvendorid = \"0463\"\n\tproductid = \"FFFF\"\n\tproduct = \"Ellipse ECO\"\n\tserial = \"000000000\"\n\tvendor = \"EATON\"\n\tbus = \"001\"\n\
                       [nutdev2]\n\tdriver = \"nutdrv_qx\"\n\tport = \"auto\"\n\tvendorid = \"0665\"\n\tproductid = \"5161\"\n\tproduct = \"\"\n";
        let devices = found(listing);
        assert_eq!(devices.len(), 2);
        assert_eq!(
            devices[0],
            FoundUps {
                driver: "usbhid-ups".into(),
                vendor_id: Some("0463".into()),
                product_id: Some("FFFF".into()),
                vendor: Some("EATON".into()),
                product: Some("Ellipse ECO".into()),
                serial: Some("000000000".into()),
            }
        );
        assert_eq!(devices[1].driver, "nutdrv_qx");
        assert_eq!(devices[1].product, None, "an empty value is none");
        assert_eq!(found(""), []);
        assert_eq!(found("No USB device found\n"), []);
    }
}
