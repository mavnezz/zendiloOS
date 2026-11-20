//! The services the package ships, as files: what systemd makes of their
//! system call filters. The first `SystemCallFilter=` sets what is allowed
//! at all: an allow-list there, and a deny-list after it takes calls out of
//! it. The other way round, the allow-list only empties the deny-list, and
//! nothing is filtered.
//!
//! And what of `/etc` the system agent may write: every place below it in
//! the agent's paths lies in a folder of its `ReadWritePaths=`, or is a
//! file directly in `/etc` that a unit of its own writes, or is only read;
//! each folder there is one the agent writes, and one the package or a
//! package it depends on makes, so that it is there when the agent starts.

use std::fs;
use std::path::{Path, PathBuf};

use zendiloos_agent::{ETC_FILES, Paths};

/// The services with a system call filter.
const FILTERED: [&str; 3] = ["zendiloosd.service", "zendiloos-agent.service", "zendiloos-files.service"];

fn units() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packaging/systemd")
}

/// The values of the unit's `SystemCallFilter=` lines, in their order.
fn filters(unit: &str) -> Vec<String> {
    let text = fs::read_to_string(units().join(unit)).expect("a unit the package ships");
    text.lines().filter_map(|line| line.trim().strip_prefix("SystemCallFilter=")).map(str::to_owned).collect()
}

#[test]
fn every_filtered_service_starts_with_an_allow_list() {
    for unit in FILTERED {
        let filters = filters(unit);
        assert!(!filters.is_empty(), "{unit} filters no system calls");
        assert!(!filters[0].starts_with('~'), "{unit}: its first SystemCallFilter= is a deny-list");
    }
    // Every service with a filter is among those looked at.
    for entry in fs::read_dir(units()).expect("the package's units") {
        let path = entry.expect("an entry").path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        if name.ends_with(".service") && !filters(name).is_empty() {
            assert!(FILTERED.contains(&name), "a service with a system call filter is not looked at");
        }
    }
}

#[test]
fn the_files_agent_may_connect_to_no_socket() {
    let filters = filters("zendiloos-files.service");
    assert_eq!(filters.first().map(String::as_str), Some("@system-service"));
    assert!(filters[1..].iter().any(|filter| filter == "~connect:EPERM"), "connect() is not taken out");
}

/// The values of the unit's `DeviceAllow=` lines.
fn devices_allowed(unit: &str) -> Vec<String> {
    let text = fs::read_to_string(units().join(unit)).expect("a unit the package ships");
    text.lines().filter_map(|line| line.trim().strip_prefix("DeviceAllow=")).map(str::to_owned).collect()
}

/// An optical drive or an SD/eMMC card the agent may not open counts as in
/// use, also when it is empty, so its USB device or controller could go to
/// no VM: the agent opens them, read only, for the exclusive probe alone.
#[test]
fn the_agent_may_open_optical_drives_and_memory_cards_read_only() {
    let allowed = devices_allowed("zendiloos-agent.service");
    for class in ["block-sr r", "block-mmc r"] {
        assert!(allowed.iter().any(|line| line == class), "the agent may not open {class}");
    }
    assert!(!allowed.iter().any(|line| line.starts_with("block-sr rw") || line.starts_with("block-mmc rw")));
}

/// The places below `/etc` the agent only reads; what writes them runs
/// outside its sandbox.
const READ_ONLY: [&str; 5] = [
    // The shadow tools, in units of their own.
    "/etc/passwd",
    "/etc/group",
    // systemd-hostnamed.
    "/etc/hostname",
    // systemd-timedated.
    "/etc/localtime",
    // libvirtd (virsh autostart --disable).
    "/etc/libvirt/qemu/autostart",
];

/// Folders the agent may write that no place in its paths names: what
/// the programs it runs write there.
const WRITTEN_BY_PROGRAMS: [&str; 1] = [
    // The ZFS module rewrites zpool.cache in the mount namespace of the
    // zpool that asks it to.
    "/etc/zfs",
];

/// Folders a package zendiloOS depends on brings, with that package:
/// there whenever the agent starts.
const BROUGHT: [(&str, &str); 6] = [
    ("/etc/zendiloos", "zendiloos"),
    ("/etc/systemd/system", "systemd"),
    ("/etc/systemd/network", "systemd"),
    ("/etc/tmpfiles.d", "systemd"),
    ("/etc/samba", "samba"),
    ("/etc/zfs", "zfsutils-linux"),
];

/// Folders neither brought nor made: there only where their work can be
/// done (ifupdown's, for a takeover of the network).
const WHERE_THERE: [&str; 1] = ["/etc/network"];

fn read(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").join(path)).expect(path)
}

/// The system agent's `ReadWritePaths=` below `/etc`, each with whether it
/// may be missing (`-`).
fn writable_in_etc() -> Vec<(String, bool)> {
    let text = fs::read_to_string(units().join("zendiloos-agent.service")).expect("the agent's unit");
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("ReadWritePaths="))
        .flat_map(str::split_whitespace)
        .map(|path| match path.strip_prefix('-') {
            Some(path) => (path.to_owned(), true),
            None => (path.to_owned(), false),
        })
        .filter(|(path, _)| path == "/etc" || path.starts_with("/etc/"))
        .collect()
}

/// The places below `/etc` in the agent's paths, as on a machine.
fn places_in_etc() -> Vec<String> {
    let paths = format!("{:?}", Paths::default());
    let places: Vec<String> =
        paths.split('"').filter(|part| part.starts_with("/etc/")).map(str::to_owned).collect();
    assert!(places.len() > 20, "the paths' Debug no longer shows them: {places:?}");
    places
}

fn below(place: &str, folder: &str) -> bool {
    place.strip_prefix(folder).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The folders below `/etc` the package makes (zendiloos.tmpfiles), with
/// their mode, owner and group.
fn made() -> Vec<(String, String)> {
    read("debian/zendiloos.tmpfiles")
        .lines()
        .filter_map(|line| line.strip_prefix("d /etc/"))
        .map(|rest| {
            let (path, how) = rest.split_once(' ').unwrap_or((rest, ""));
            (format!("/etc/{path}"), how.trim().to_owned())
        })
        .collect()
}

/// The packages zendiloOS depends on, by name (debian/control).
fn depends() -> Vec<String> {
    let control = read("debian/control");
    let (_, rest) = control.split_once("\nDepends:").expect("Depends:");
    // Its lines go on indented, up to the next field.
    let field: String = rest
        .lines()
        .enumerate()
        .take_while(|(number, line)| *number == 0 || line.starts_with(' '))
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join(" ");
    field
        .split(',')
        .filter_map(|alternatives| alternatives.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_agent_may_write_every_place_below_etc_it_writes_and_no_more() {
    let writable = writable_in_etc();
    assert!(!writable.iter().any(|(path, _)| path == "/etc"), "/etc is writable as a whole");
    for place in places_in_etc() {
        let path = Path::new(&place);
        let in_etc = path.parent() == Some(Path::new("/etc"));
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let written = writable.iter().any(|(folder, _)| below(&place, folder));
        let by_unit = in_etc && ETC_FILES.contains(&name);
        assert!(
            written || by_unit || READ_ONLY.contains(&place.as_str()),
            "{place}: in no folder the agent may write, nor written by a unit of its own"
        );
        assert!(!(written && by_unit), "{place}: written by a unit, but in a folder the agent writes");
    }
    // Every folder is one the agent writes.
    let places = places_in_etc();
    for (folder, _) in &writable {
        assert!(
            places.iter().any(|place| below(place, folder)) || WRITTEN_BY_PROGRAMS.contains(&folder.as_str()),
            "{folder}: writable, but the agent writes nothing there"
        );
    }
}

#[test]
fn every_folder_the_agent_writes_is_there_when_it_starts() {
    let made = made();
    let depends = depends();
    for (folder, optional) in writable_in_etc() {
        let brought = BROUGHT.iter().find(|(path, _)| *path == folder);
        let package_made = made.iter().find(|(path, _)| *path == folder);
        match (brought, package_made) {
            (Some((_, package)), None) => {
                assert!(!optional, "{folder}: {package} brings it, so the agent needs it to start");
                assert!(
                    *package == "zendiloos" || depends.iter().any(|name| name == package),
                    "{folder}: zendiloOS does not depend on {package}"
                );
            }
            (None, Some((_, how))) => {
                assert!(
                    optional,
                    "{folder}: the agent should start without it, as before the package made it"
                );
                // As Debian's packages make it, and one that is there is
                // left as it is, owner and mode too.
                assert_eq!(how, "- - - -", "{folder}: its mode or owner set");
            }
            (None, None) => {
                assert!(WHERE_THERE.contains(&folder.as_str()), "{folder}: nothing makes it");
                assert!(optional, "{folder}: the agent should start without it");
            }
            (Some(_), Some(_)) => panic!("{folder}: both brought by a package and made"),
        }
    }
    // Nothing made that the agent does not write: what it made for the web
    // server's certificates aside, in /etc/zendiloos.
    let writable = writable_in_etc();
    for (folder, _) in made {
        assert!(
            writable.iter().any(|(path, _)| below(&folder, path)),
            "{folder}: made by the package, but the agent may not write it"
        );
    }
}
