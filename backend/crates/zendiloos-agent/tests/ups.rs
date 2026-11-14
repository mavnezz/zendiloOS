//! Network UPS Tools against a fake host: `systemctl` and `systemd-run`
//! replaced by scripts that record their arguments, NUT's folder in a
//! temporary directory.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, Operations, Paths, UpsPaths};
use zendiloos_agent_proto::{
    ErrorCode, Request, Response, Role, SnmpVersion, UpsAccount, UpsConfig, UpsHost, UpsServerPort,
    UpsShutdown, UpsSource, UpsWord, UsbDriver, UsbId,
};

struct Host {
    _dir: TempDir,
    root: PathBuf,
    agent: Operations,
}

/// What the fake scanner prints: an Eaton UPS on USB.
const LISTING: &str = "[nutdev1]\n\tdriver = \"usbhid-ups\"\n\tport = \"auto\"\n\tvendorid = \"0463\"\n\tproductid = \"FFFF\"\n\tproduct = \"Ellipse ECO\"\n\tvendor = \"EATON\"\n";

impl Host {
    /// A host with NUT installed (`installed`) and, with it, its SNMP
    /// driver (`snmp`).
    fn new(installed: bool, snmp: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let log = root.join("calls.log");
        let script = |name: &str, body: &str| -> PathBuf {
            let path = root.join(name);
            fs::write(&path, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", log.display()))
                .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let nut = root.join("nut");
        fs::create_dir(&nut).unwrap();
        let missing = root.join("missing");
        let tools = Tools {
            // The driver's unit does not start: no UPS is plugged in.
            systemctl: script(
                "systemctl",
                &format!(
                    "case \"$*\" in\n  'show --property=NeedDaemonReload'*) echo no;;\n  *' --no-block nut-driver@ups.service') exit 1;;\n  \
                     is-active*) [ ! -e {} ] || exit 3;;\nesac",
                    root.join("stopped").display()
                ),
            ),
            systemd_run: script("systemd-run", &format!("printf '%s' '{LISTING}'")),
            upsd: if installed { script("upsd", "") } else { missing.clone() },
            upsmon: if installed { script("upsmon", "") } else { missing.clone() },
            nut_scanner: if installed { script("nut-scanner", "") } else { missing.clone() },
            ..Tools::default()
        };
        let snmp_driver = root.join("snmp-ups");
        if snmp {
            fs::write(&snmp_driver, "").unwrap();
        }
        let paths = Paths { ups: UpsPaths { dir: nut, snmp_driver }, ..Paths::default() };
        let agent =
            Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::Unused), paths)
                .unwrap();
        Self { _dir: dir, root, agent }
    }

    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.root.join("calls.log")).unwrap_or_default();
        log.lines().map(str::to_owned).collect()
    }

    fn file(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.root.join("nut").join(name)).ok()
    }

    fn mode(&self, name: &str) -> u32 {
        fs::metadata(self.root.join("nut").join(name)).unwrap().permissions().mode() & 0o777
    }

    async fn set(&self, config: Option<UpsConfig>) -> Response {
        self.agent.execute(&Request::SetUps { config }).await
    }
}

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
        product_id: Some(UsbId::parse("ffff").unwrap()),
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

#[tokio::test]
async fn the_facts_say_what_of_nut_is_there() {
    let host = Host::new(true, false);
    assert!(matches!(
        host.agent.execute(&Request::UpsFacts {}).await,
        Response::UpsFacts { installed: true, snmp: false }
    ));
    let bare = Host::new(false, false);
    assert!(matches!(
        bare.agent.execute(&Request::UpsFacts {}).await,
        Response::UpsFacts { installed: false, snmp: false }
    ));
    assert!(bare.calls().is_empty(), "looked for, nothing run");
}

#[tokio::test]
async fn a_usb_ups_gets_its_files_driver_server_and_monitor() {
    let host = Host::new(true, false);
    let served = UpsConfig {
        shutdown: UpsShutdown::AfterMinutes { minutes: 5 },
        cut_power: true,
        serve: Some(UpsAccount { user: word("monuser"), password: word("for-others") }),
        ..config(usb())
    };
    assert!(matches!(host.set(Some(served)).await, Response::Done));

    assert_eq!(host.file("nut.conf").unwrap().lines().last(), Some("MODE=netserver"));
    let ups = host.file("ups.conf").unwrap();
    assert!(ups.contains(
        "[ups]\n    driver = usbhid-ups\n    port = auto\n    vendorid = 0463\n    productid = ffff\n"
    ));
    assert!(host.file("upsd.conf").unwrap().ends_with("LISTEN 0.0.0.0 3493\n"));
    let users = host.file("upsd.users").unwrap();
    assert!(users.contains("[upsmon_local]\n    password = local-pw\n    upsmon primary\n"));
    assert!(users.contains("[monuser]\n    password = for-others\n    upsmon secondary\n"));
    let monitor = host.file("upsmon.conf").unwrap();
    assert!(monitor.contains("MONITOR ups@localhost 1 upsmon_local local-pw primary\n"));
    assert!(monitor.contains("POWERDOWNFLAG /etc/killpower\n"));
    assert!(monitor.contains("NOTIFYCMD /usr/sbin/upssched\n"));
    let timer = host.root.join("nut/zendiloos-timer");
    let sched = host.file("upssched.conf").unwrap();
    assert!(sched.contains(&format!("CMDSCRIPT {}\n", timer.display())));
    assert!(sched.contains("AT ONBATT * START-TIMER onbatt 300\n"));
    assert!(host.file("zendiloos-timer").unwrap().contains("upsmon -c fsd"));
    // The files hold passwords: for root and NUT's group only; the script runs.
    for name in ["nut.conf", "ups.conf", "upsd.conf", "upsd.users", "upsmon.conf", "upssched.conf"] {
        assert_eq!(host.mode(name), 0o640, "{name}");
    }
    assert_eq!(host.mode("zendiloos-timer"), 0o750);
    assert_eq!(host.file("zendiloos-simulated.dev"), None, "no simulated UPS");

    // The driver's unit cannot be started (nothing is plugged in); the rest goes on.
    assert_eq!(
        host.calls(),
        [
            "systemctl restart --no-block nut-driver-enumerator.service",
            "systemctl enable --no-reload nut-server.service nut-monitor.service",
            "systemctl show --property=NeedDaemonReload --value nut-server.service",
            "systemctl restart --no-block nut-driver@ups.service",
            "systemctl restart nut-server.service",
            "systemctl restart nut-monitor.service",
        ]
    );
}

#[tokio::test]
async fn another_devices_ups_needs_no_driver_and_no_server() {
    let host = Host::new(true, false);
    assert!(matches!(host.set(Some(config(server()))).await, Response::Done));
    assert_eq!(host.file("nut.conf").unwrap().lines().last(), Some("MODE=netclient"));
    assert!(!host.file("ups.conf").unwrap().contains("[ups]"));
    assert!(!host.file("upsd.users").unwrap().contains("password"));
    assert!(
        host.file("upsmon.conf")
            .unwrap()
            .contains("MONITOR ups@192.168.1.9:3493 1 monuser secret secondary\n")
    );
    assert_eq!(
        host.calls(),
        [
            "systemctl restart --no-block nut-driver-enumerator.service",
            "systemctl disable --no-reload --now nut-server.service",
            "systemctl enable --no-reload nut-monitor.service",
            "systemctl show --property=NeedDaemonReload --value nut-monitor.service",
            "systemctl restart nut-monitor.service",
        ]
    );
}

#[tokio::test]
async fn without_a_ups_nut_is_switched_off() {
    let host = Host::new(true, false);
    assert!(matches!(host.set(Some(config(UpsSource::Simulated))).await, Response::Done));
    assert!(host.file("zendiloos-simulated.dev").unwrap().starts_with("ups.status: OL\n"));
    assert!(host.file("upsd.users").unwrap().contains("actions = SET"));
    fs::write(host.root.join("calls.log"), "").unwrap();

    assert!(matches!(host.set(None).await, Response::Done));
    assert_eq!(host.file("nut.conf").unwrap().lines().last(), Some("MODE=none"));
    assert!(!host.file("ups.conf").unwrap().contains("[ups]"));
    assert!(!host.file("upsmon.conf").unwrap().contains("MONITOR"));
    assert_eq!(host.file("zendiloos-simulated.dev"), None, "the played UPS goes with its settings");
    assert_eq!(
        host.calls(),
        [
            "systemctl disable --no-reload --now nut-monitor.service nut-server.service",
            "systemctl show --property=NeedDaemonReload --value nut-server.service",
            "systemctl restart --no-block nut-driver-enumerator.service",
            "systemctl stop --no-block nut-driver@ups.service",
        ]
    );

    // Where NUT is not installed there is nothing to switch off.
    let bare = Host::new(false, false);
    assert!(matches!(bare.set(None).await, Response::Done));
    assert!(bare.calls().is_empty());
    assert_eq!(bare.file("nut.conf"), None);
}

#[tokio::test]
async fn settings_in_force_already_interrupt_nothing() {
    let host = Host::new(true, false);
    assert!(matches!(host.set(Some(config(usb()))).await, Response::Done));
    fs::write(host.root.join("calls.log"), "").unwrap();

    // The same again, as at every start of zendiloosd: only looked at.
    assert!(matches!(host.set(Some(config(usb()))).await, Response::Done));
    assert_eq!(
        host.calls(),
        ["systemctl is-active --quiet nut-server.service", "systemctl is-active --quiet nut-monitor.service",]
    );
    // Unless NUT does not run, as after someone stopped it: started again,
    // the driver too, which is not started anew for a section that stayed.
    fs::write(host.root.join("stopped"), "").unwrap();
    assert!(matches!(host.set(Some(config(usb()))).await, Response::Done));
    let calls = host.calls();
    assert!(calls.contains(&"systemctl restart nut-monitor.service".to_owned()), "{calls:?}");
    assert!(calls.contains(&"systemctl start --no-block nut-driver@ups.service".to_owned()), "{calls:?}");
    fs::remove_file(host.root.join("stopped")).unwrap();

    // Another rule for the shutdown that the driver has to know: started
    // anew, as its live reload would leave the old rule in force.
    fs::write(host.root.join("calls.log"), "").unwrap();
    let charge = UpsConfig { shutdown: UpsShutdown::ChargeBelow { percent: 40 }, ..config(usb()) };
    assert!(matches!(host.set(Some(charge.clone())).await, Response::Done));
    assert!(host.file("ups.conf").unwrap().contains("    ignorelb\n    override.battery.charge.low = 40\n"));
    let calls = host.calls();
    assert!(calls.contains(&"systemctl restart --no-block nut-driver@ups.service".to_owned()), "{calls:?}");
    // One the driver has nothing to do with: the monitor's alone.
    fs::write(host.root.join("calls.log"), "").unwrap();
    let cut = UpsConfig { cut_power: true, ..charge };
    assert!(matches!(host.set(Some(cut)).await, Response::Done));
    let calls = host.calls();
    assert!(calls.contains(&"systemctl start --no-block nut-driver@ups.service".to_owned()), "{calls:?}");
    assert!(calls.contains(&"systemctl restart nut-monitor.service".to_owned()), "{calls:?}");

    // No UPS, asked for twice: switched off once.
    assert!(matches!(host.set(None).await, Response::Done));
    fs::write(host.root.join("stopped"), "").unwrap();
    fs::write(host.root.join("calls.log"), "").unwrap();
    assert!(matches!(host.set(None).await, Response::Done));
    assert_eq!(host.calls(), ["systemctl is-active --quiet nut-monitor.service nut-server.service"]);
}

#[tokio::test]
async fn a_nut_set_up_by_hand_is_left_alone_until_a_ups_is_set_and_then_kept_beside() {
    let host = Host::new(true, false);
    let by_hand = "MODE=standalone\n";
    fs::write(host.root.join("nut/nut.conf"), by_hand).unwrap();
    fs::write(host.root.join("nut/ups.conf"), "[apc]\n  driver = apcsmart\n  port = /dev/ttyS0\n").unwrap();

    // No UPS in zendiloOS: not its NUT to switch off.
    assert!(matches!(host.set(None).await, Response::Done));
    assert_eq!(host.file("nut.conf").as_deref(), Some(by_hand));
    assert!(host.calls().is_empty());

    // A UPS set in zendiloOS takes the files; what they said stays beside them.
    assert!(matches!(host.set(Some(config(usb()))).await, Response::Done));
    assert!(host.file("ups.conf").unwrap().contains("driver = usbhid-ups"));
    assert_eq!(host.file("nut.conf.before-zendiloos").as_deref(), Some(by_hand));
    assert!(host.file("ups.conf.before-zendiloos").unwrap().contains("apcsmart"));
    assert_eq!(host.mode("ups.conf.before-zendiloos"), 0o640);
    assert_eq!(host.file("upsd.users.before-zendiloos"), None, "there was none");
    // Once only: zendiloOS's own files are not kept as what was before.
    assert!(matches!(host.set(Some(config(UpsSource::Simulated))).await, Response::Done));
    assert!(host.file("ups.conf.before-zendiloos").unwrap().contains("apcsmart"));
    // Now it is zendiloOS's to switch off.
    assert!(matches!(host.set(None).await, Response::Done));
    assert_eq!(host.file("nut.conf").unwrap().lines().last(), Some("MODE=none"));
}

#[tokio::test]
async fn what_is_missing_or_does_not_fit_writes_nothing() {
    let bare = Host::new(false, false);
    let refused = bare.set(Some(config(usb()))).await;
    assert!(
        matches!(&refused, Response::Error { message, .. } if message.contains("not installed")),
        "{refused:?}"
    );
    assert_eq!(bare.file("ups.conf"), None);

    let host = Host::new(true, false);
    let snmp = config(UpsSource::Snmp {
        address: UpsHost::parse("10.0.0.9").unwrap(),
        community: word("public"),
        version: SnmpVersion::V2c,
    });
    let refused = host.set(Some(snmp.clone())).await;
    assert!(
        matches!(&refused, Response::Error { message, .. } if message.contains("nut-snmp")),
        "{refused:?}"
    );
    // A UPS at another device cannot be told to cut its output.
    let refused = host.set(Some(UpsConfig { cut_power: true, ..config(server()) })).await;
    assert!(
        matches!(&refused, Response::Error { code: ErrorCode::Forbidden, message } if message.starts_with("refused: ")),
        "{refused:?}"
    );
    assert_eq!(host.file("ups.conf"), None);
    assert!(host.calls().is_empty());

    // With the driver there, SNMP is set up like any UPS of the NAS's own.
    let with_snmp = Host::new(true, true);
    assert!(matches!(with_snmp.set(Some(snmp)).await, Response::Done));
    assert!(with_snmp.file("ups.conf").unwrap().contains("driver = snmp-ups\n    port = 10.0.0.9\n"));
}

#[tokio::test]
async fn the_scanner_runs_in_a_unit_of_its_own_and_its_devices_come_back() {
    let host = Host::new(true, false);
    let found = host.agent.execute(&Request::ScanUps {}).await;
    let Response::UpsFound { devices } = found else { panic!("{found:?}") };
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].driver, "usbhid-ups");
    assert_eq!(devices[0].product.as_deref(), Some("Ellipse ECO"));
    let scanner = host.root.join("nut-scanner").display().to_string();
    assert_eq!(host.calls(), [format!("systemd-run --wait --pipe --collect --quiet {scanner} -U -q")]);

    let bare = Host::new(false, false);
    assert!(matches!(bare.agent.execute(&Request::ScanUps {}).await, Response::Error { .. }));
}
