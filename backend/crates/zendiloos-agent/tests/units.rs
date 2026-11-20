//! The services the package ships, as files: what systemd makes of their
//! system call filters. The first `SystemCallFilter=` sets what is allowed
//! at all: an allow-list there, and a deny-list after it takes calls out of
//! it. The other way round, the allow-list only empties the deny-list, and
//! nothing is filtered.

use std::fs;
use std::path::{Path, PathBuf};

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
