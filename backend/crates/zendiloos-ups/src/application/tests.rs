use zendiloos_jobs::{Job, JobState};
use zendiloos_kernel::{Actor, RequestContext};

use super::*;
use crate::domain::{Account, MISSES_BEFORE_UNREACHABLE};
use crate::testing::{Fakes, LOCAL_PASSWORD, ups};

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn context() -> RequestContext {
    RequestContext::default()
}

fn usb() -> SourceInput {
    SourceInput::Usb {
        driver: "usbhid-ups".into(),
        vendor_id: Some("0463".into()),
        product_id: Some("FFFF".into()),
        serial: None,
    }
}

fn server(password: Option<&str>) -> SourceInput {
    SourceInput::Server {
        host: "nas.local".into(),
        port: 3493,
        ups: "ups".into(),
        user: "monuser".into(),
        password: password.map(str::to_owned),
    }
}

fn input(source: Option<SourceInput>) -> SettingsInput {
    SettingsInput { source, shutdown: ShutdownWhen::LowBattery, cut_power: false, serve: None }
}

fn codes(fakes: &Fakes) -> Vec<String> {
    fakes.notifications.all().into_iter().map(|notification| notification.code).collect()
}

async fn finished(fakes: &Fakes, job: &Job) -> Job {
    for _ in 0..1000 {
        if let Some(found) = fakes.jobs.all().into_iter().find(|j| j.id == job.id)
            && found.state != JobState::Running
        {
            return found;
        }
        tokio::task::yield_now().await;
    }
    panic!("the job did not end");
}

#[tokio::test]
async fn without_a_ups_the_page_shows_what_is_installed_and_asks_nobody() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);
    let overview = ups.overview(&context()).await.unwrap();
    assert_eq!(overview.facts, HostFacts { installed: true, snmp: true });
    assert_eq!(overview.settings, Settings::default());
    assert_eq!(overview.state, None);
    assert!(fakes.link.asked.lock().unwrap().is_empty());
    assert!(!ups.serving().await.unwrap());
}

#[tokio::test]
async fn a_usb_ups_is_put_in_force_kept_and_read_from_the_nas_itself() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);
    fakes.link.says("OL CHRG");

    let overview = ups.set(input(Some(usb())), &alice(), &context()).await.unwrap();

    let applied = fakes.host.applied.lock().unwrap().clone();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].local_password, LOCAL_PASSWORD);
    assert!(
        matches!(&applied[0].settings.source, Some(Source::Usb { product_id: Some(id), .. }) if id == "ffff"),
        "checked before it reaches the host"
    );
    assert_eq!(*fakes.store.0.lock().unwrap(), applied[0].settings);
    let state = overview.state.unwrap();
    assert!(!state.on_battery && state.charging);
    assert_eq!(state.charge_percent, Some(80));
    assert_eq!(
        fakes.link.asked.lock().unwrap().last().unwrap(),
        &Target { host: "127.0.0.1".into(), port: 3493, ups: "ups".into() }
    );
    let audit = fakes.audit.0.lock().unwrap();
    assert_eq!(
        (audit[0].action, audit[0].outcome, audit[0].target.as_deref()),
        ("ups.update", Outcome::Success, Some("usb"))
    );
}

#[tokio::test]
async fn what_is_wrong_or_refused_is_neither_applied_nor_kept() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);

    let wrong = SettingsInput { shutdown: ShutdownWhen::ChargeBelow(2), ..input(Some(usb())) };
    assert!(matches!(ups.set(wrong, &alice(), &context()).await, Err(UpsError::Invalid(Invalid::Charge))));
    assert!(fakes.host.applied.lock().unwrap().is_empty());

    *fakes.host.refuses.lock().unwrap() = Some("the driver's file is missing".into());
    let refused = ups.set(input(Some(usb())), &alice(), &context()).await;
    assert!(matches!(&refused, Err(UpsError::Refused(why)) if why.contains("driver")), "{refused:?}");
    assert_eq!(*fakes.store.0.lock().unwrap(), Settings::default(), "nothing kept");
    assert_eq!(fakes.audit.actions(), ["ups.update", "ups.update"]);
    assert!(fakes.audit.0.lock().unwrap().iter().all(|entry| entry.outcome == Outcome::Failure));
}

#[tokio::test]
async fn a_ups_needs_nut_and_snmp_its_driver() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);
    *fakes.host.facts.lock().unwrap() = HostFacts { installed: false, snmp: false };
    assert!(matches!(ups.set(input(Some(usb())), &alice(), &context()).await, Err(UpsError::NotInstalled)));
    assert!(matches!(ups.scan(&context()).await, Err(UpsError::NotInstalled)));
    // Without NUT, "no UPS" is kept all the same: there is nothing to stop.
    assert!(ups.set(input(None), &alice(), &context()).await.is_ok());
    assert!(fakes.host.applied.lock().unwrap().is_empty());

    *fakes.host.facts.lock().unwrap() = HostFacts { installed: true, snmp: false };
    let snmp = SourceInput::Snmp {
        address: "10.0.0.9".into(),
        community: "public".into(),
        version: SnmpVersion::V2c,
    };
    assert!(matches!(ups.set(input(Some(snmp)), &alice(), &context()).await, Err(UpsError::SnmpMissing)));
}

#[tokio::test]
async fn a_password_left_out_stays_the_one_kept_for_the_same_account() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);

    // None is kept yet: it has to be typed.
    assert!(matches!(
        ups.set(input(Some(server(None))), &alice(), &context()).await,
        Err(UpsError::PasswordNeeded)
    ));
    ups.set(input(Some(server(Some("secret")))), &alice(), &context()).await.unwrap();
    // Changed without typing it again.
    let later = SettingsInput { shutdown: ShutdownWhen::AfterMinutes(10), ..input(Some(server(None))) };
    let overview = ups.set(later, &alice(), &context()).await.unwrap();
    assert!(
        matches!(&overview.settings.source, Some(Source::Server { password, .. }) if password == "secret")
    );
    assert_eq!(
        fakes.link.asked.lock().unwrap().last().unwrap(),
        &Target { host: "nas.local".into(), port: 3493, ups: "ups".into() },
        "its state is read from the other device"
    );
    // Another account, or another host, does not get the old password.
    let other = SourceInput::Server {
        host: "other.local".into(),
        port: 3493,
        ups: "ups".into(),
        user: "monuser".into(),
        password: None,
    };
    assert!(matches!(ups.set(input(Some(other)), &alice(), &context()).await, Err(UpsError::PasswordNeeded)));

    // The same for the account other devices follow with.
    let serve = |user: &str, password: Option<&str>| SettingsInput {
        serve: Some(AccountInput { user: user.into(), password: password.map(str::to_owned) }),
        ..input(Some(usb()))
    };
    assert!(matches!(
        ups.set(serve("monuser", None), &alice(), &context()).await,
        Err(UpsError::PasswordNeeded)
    ));
    ups.set(serve("monuser", Some("for-others")), &alice(), &context()).await.unwrap();
    assert!(ups.serving().await.unwrap());
    let kept = ups.set(serve("monuser", Some("")), &alice(), &context()).await.unwrap();
    assert_eq!(kept.settings.serve, Some(Account { user: "monuser".into(), password: "for-others".into() }));
    assert!(matches!(
        ups.set(serve("another", None), &alice(), &context()).await,
        Err(UpsError::PasswordNeeded)
    ));
}

#[tokio::test]
async fn nut_is_installed_in_a_job_and_settings_that_wait_are_put_in_force() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);
    assert!(matches!(ups.install(&alice(), &context()).await, Err(UpsError::Installed)));

    // As after a configuration was restored on a fresh installation.
    *fakes.host.facts.lock().unwrap() = HostFacts { installed: false, snmp: false };
    let waiting = Settings {
        source: Some(Source::Usb {
            driver: "usbhid-ups".into(),
            vendor_id: None,
            product_id: None,
            serial: None,
        }),
        ..Settings::default()
    };
    *fakes.store.0.lock().unwrap() = waiting.clone();

    let job = ups.install(&alice(), &context()).await.unwrap();
    assert_eq!((job.kind.as_str(), job.target.as_deref()), ("ups.install", Some("nut")));
    assert_eq!(finished(&fakes, &job).await.state, JobState::Succeeded);
    assert_eq!(*fakes.host.facts.lock().unwrap(), HostFacts { installed: true, snmp: true });
    assert_eq!(
        fakes.host.applied.lock().unwrap().iter().map(|s| s.settings.clone()).collect::<Vec<_>>(),
        [waiting]
    );
    assert_eq!(fakes.audit.actions(), ["ups.install"]);
}

#[tokio::test]
async fn at_the_start_the_host_is_given_the_settings_kept() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);
    ups.set(input(Some(usb())), &alice(), &context()).await.unwrap();
    fakes.host.applied.lock().unwrap().clear();

    ups.apply_kept(&context()).await.unwrap();
    let applied = fakes.host.applied.lock().unwrap().clone();
    assert_eq!(applied.len(), 1);
    assert!(matches!(applied[0].settings.source, Some(Source::Usb { .. })));
    assert_eq!(applied[0].local_password, LOCAL_PASSWORD);
    assert!(fakes.audit.actions().iter().all(|action| *action == "ups.update"), "nobody did it");

    // Without NUT nothing can follow them; what the host refuses is told.
    fakes.host.applied.lock().unwrap().clear();
    *fakes.host.facts.lock().unwrap() = HostFacts { installed: false, snmp: false };
    ups.apply_kept(&context()).await.unwrap();
    assert!(fakes.host.applied.lock().unwrap().is_empty());
    *fakes.host.facts.lock().unwrap() = HostFacts { installed: true, snmp: true };
    *fakes.host.refuses.lock().unwrap() = Some("no".into());
    assert!(matches!(ups.apply_kept(&context()).await, Err(UpsError::Refused(_))));
}

#[tokio::test]
async fn the_watch_tells_a_power_cut_its_end_and_a_ups_gone_silent() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);

    // Without a UPS nothing is asked or told.
    ups.watch().await;
    assert!(fakes.link.asked.lock().unwrap().is_empty());

    fakes.link.says("OL");
    ups.set(input(Some(usb())), &alice(), &context()).await.unwrap();
    assert!(!ups.watch().await);
    assert_eq!(codes(&fakes), [] as [&str; 0]);

    fakes.link.says("OB DISCHRG");
    assert!(ups.watch().await, "on battery: looked at again sooner");
    ups.watch().await;
    assert_eq!(codes(&fakes), ["ups_on_battery"]);
    // Told with what happens next and what the battery holds.
    let told = fakes.notifications.all().remove(0).params;
    assert_eq!(
        told.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect::<Vec<_>>(),
        [("charge", "80"), ("runtime", "20"), ("when", "low_battery")]
    );
    fakes.link.says("OB DISCHRG LB");
    ups.watch().await;
    fakes.link.says("OL CHRG");
    assert!(!ups.watch().await);
    assert_eq!(codes(&fakes), ["ups_on_battery", "ups_shutdown", "ups_on_mains"]);
    assert_eq!(fakes.notifications.all()[1].params.get("why").map(String::as_str), Some("battery"));

    fakes.link.silent();
    for _ in 0..MISSES_BEFORE_UNREACHABLE {
        ups.watch().await;
    }
    ups.watch().await;
    assert_eq!(codes(&fakes).last().map(String::as_str), Some("ups_unreachable"));
    assert_eq!(codes(&fakes).len(), 4, "told once");

    // New settings start the watch afresh: an old silence is forgotten.
    fakes.link.says("OL RB");
    ups.set(input(Some(usb())), &alice(), &context()).await.unwrap();
    ups.watch().await;
    assert_eq!(codes(&fakes).last().map(String::as_str), Some("ups_replace_battery"));
    let levels: Vec<_> = fakes.notifications.all().into_iter().map(|n| (n.code, n.level)).collect();
    assert!(levels.contains(&("ups_shutdown".into(), Level::Error)));
    assert!(levels.contains(&("ups_on_mains".into(), Level::Success)));
}

#[tokio::test]
async fn a_power_cut_is_told_with_the_rule_the_nas_shuts_down_by() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);
    let params = |fakes: &Fakes, code: &str| {
        let all = fakes.notifications.all();
        let told = all.iter().rev().find(|notification| notification.code == code).unwrap();
        told.params.iter().map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join(" ")
    };

    fakes.link.says("OL");
    let minutes = SettingsInput { shutdown: ShutdownWhen::AfterMinutes(10), ..input(Some(usb())) };
    ups.set(minutes, &alice(), &context()).await.unwrap();
    ups.watch().await;
    fakes.link.says("OB");
    ups.watch().await;
    assert_eq!(params(&fakes, "ups_on_battery"), "charge=80 minutes=10 runtime=20 when=after_minutes");
    // The time is over: a shutdown is forced while the battery still holds.
    fakes.link.says("OB FSD");
    ups.watch().await;
    assert_eq!(params(&fakes, "ups_shutdown"), "why=time");

    fakes.link.says("OL");
    let charge = SettingsInput { shutdown: ShutdownWhen::ChargeBelow(40), ..input(Some(usb())) };
    ups.set(charge, &alice(), &context()).await.unwrap();
    ups.watch().await;
    fakes.link.says("OB");
    ups.watch().await;
    assert_eq!(params(&fakes, "ups_on_battery"), "charge=80 percent=40 runtime=20 when=charge_below");
}

#[tokio::test]
async fn the_scanner_s_devices_are_handed_on() {
    let fakes = Fakes::default();
    let ups = ups(&fakes);
    let eaton = Found {
        driver: "usbhid-ups".into(),
        vendor_id: Some("0463".into()),
        product_id: Some("FFFF".into()),
        vendor: Some("EATON".into()),
        product: Some("Ellipse ECO".into()),
        serial: None,
    };
    *fakes.host.found.lock().unwrap() = vec![eaton.clone()];
    assert_eq!(ups.scan(&context()).await.unwrap(), [eaton]);
}
