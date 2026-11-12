use super::*;
use crate::domain::{DeviceKind, Ipv4Setting, NetworkdFile};

/// A file as the agent writes it.
fn file(name: &str, text: &str) -> NetworkdFile {
    NetworkdFile { name: format!("05-zendiloos-{name}"), text: format!("# Written by zendiloOS.\n{text}") }
}

const BRIDGE: &str =
    "# Members: enp1s0\n[NetDev]\nName=br0\nKind=bridge\nMACAddress=52:54:00:12:34:56\n\n[Bridge]\nSTP=no\n";
const BR0: &str = "[Match]\nName=br0\n\n[Link]\nRequiredForOnline=no\n\n[Network]\nDHCP=ipv4\nIPv6AcceptRA=yes\n\n\
    [DHCPv4]\nClientIdentifier=mac\nUseDomains=yes\nRequestAddress=192.168.1.20\n";
const PORT: &str =
    "[Match]\nName=enp1s0\n\n[Network]\nBridge=br0\nDHCP=no\nIPv6AcceptRA=no\nLinkLocalAddressing=no\n";
const FIXED: &str = "[Match]\nName=enp2s0\n\n[Network]\nDHCP=no\nIPv6AcceptRA=yes\nAddress=10.0.0.5/24\nGateway=10.0.0.1\nDNS=10.0.0.1\n";

/// A bridge over enp1s0 with the NAS's address by DHCP, and enp2s0 fixed.
fn confirmed() -> Vec<NetworkdFile> {
    vec![
        file("br0.netdev", BRIDGE),
        file("br0.network", BR0),
        file("enp1s0.network", PORT),
        file("enp2s0.network", FIXED),
    ]
}

fn read(files: &[NetworkdFile]) -> Configuration {
    Configuration::read(files)
}

fn difference(part: Part, name: &str, kind: DifferenceKind) -> Difference {
    Difference { part, name: name.into(), kind }
}

#[test]
fn the_same_settings_written_otherwise_differ_in_nothing() {
    let kept = read(&confirmed());
    // Another header, comments, blank lines and keys zendiloOS does not read.
    let rewritten: Vec<NetworkdFile> = confirmed()
        .into_iter()
        .map(|file| NetworkdFile {
            text: file.text.replace("# Written by zendiloOS.\n", "# zendiloOS 0.9\n\n").replace("\n\n", "\n"),
            ..file
        })
        .collect();
    assert_eq!(differences(&kept, &read(&rewritten)), []);
    assert_eq!(restoring(&kept, &read(&rewritten)), None);
}

#[test]
fn files_lost_changed_or_added_outside_zendiloos_are_told() {
    let kept = read(&confirmed());
    let mut now = confirmed();
    // The bridge's .netdev lost, enp2s0 changed to DHCP, a VLAN's file and
    // enp3s0's written by hand.
    now.retain(|file| !file.name.ends_with("br0.netdev"));
    now.iter_mut().find(|file| file.name.ends_with("enp2s0.network")).unwrap().text =
        "[Match]\nName=enp2s0\n[Network]\nDHCP=ipv4\n".into();
    now.push(file("enp1s0.10.netdev", "[NetDev]\nName=enp1s0.10\nKind=vlan\n[VLAN]\nId=10\n"));
    now.push(file("enp3s0.network", "[Match]\nName=enp3s0\n[Network]\nDHCP=ipv4\n"));
    let found = differences(&kept, &read(&now));
    assert_eq!(
        found,
        [
            difference(Part::Device, "br0", DifferenceKind::Missing),
            difference(Part::Device, "enp1s0.10", DifferenceKind::Added),
            // A device no longer, br0's own file is up without addresses no longer.
            difference(Part::Interface, "br0", DifferenceKind::Changed),
            difference(Part::Interface, "enp2s0", DifferenceKind::Changed),
            difference(Part::Interface, "enp3s0", DifferenceKind::Added),
        ]
    );

    let edit = restoring(&kept, &read(&now)).unwrap();
    assert_eq!(edit.subject, Subject::Restore);
    assert_eq!(edit.devices, [kept.device("br0").unwrap().clone()], "the bridge written as kept");
    assert!(matches!(edit.devices[0].kind, DeviceKind::Bridge { stp: false }));
    assert_eq!(edit.removed, ["enp1s0.10"], "the device made outside goes");
    let written: Vec<&str> = edit.links.iter().map(|link| link.settings.interface.as_str()).collect();
    assert_eq!(written, ["br0", "enp2s0"]);
    assert!(matches!(edit.links[1].settings.ipv4, Ipv4Setting::Static { .. }));
    assert_eq!(edit.unset, ["enp3s0"], "the file written outside goes");
    assert!(edit.effects.is_empty());
}

/// Written back, the files say the settings again.
#[test]
fn a_restore_leaves_nothing_that_differs() {
    let kept = read(&confirmed());
    let now = vec![file("enp2s0.network", "[Match]\nName=enp2s0\n[Network]\nDHCP=ipv4\n")];
    let edit = restoring(&kept, &read(&now)).unwrap();
    // As the agent would leave them: each written file as kept, the
    // others gone.
    let mut after: Vec<NetworkdFile> = confirmed()
        .into_iter()
        .filter(|file| {
            edit.devices.iter().any(|device| file.name.ends_with(&format!("{}.netdev", device.name)))
                || edit
                    .links
                    .iter()
                    .any(|link| file.name.ends_with(&format!("{}.network", link.settings.interface)))
        })
        .collect();
    after.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(differences(&kept, &read(&after)), []);
}

#[test]
fn a_device_made_outside_goes_with_its_own_file() {
    let kept = read(&[file("enp2s0.network", FIXED)]);
    let mut now = vec![file("enp2s0.network", FIXED)];
    now.push(file("br9.netdev", "[NetDev]\nName=br9\nKind=bridge\n"));
    now.push(file("br9.network", "[Match]\nName=br9\n[Network]\nDHCP=ipv4\n"));
    let edit = restoring(&kept, &read(&now)).unwrap();
    assert_eq!(edit.removed, ["br9"]);
    assert!(edit.unset.is_empty(), "br9's .network goes with it");
    assert!(edit.links.is_empty() && edit.devices.is_empty());
}
