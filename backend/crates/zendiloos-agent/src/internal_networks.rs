//! Internal networks of virtual machines: bridges zendiloOS made without
//! members and without an address of the NAS's, which the NAS keeps itself
//! off (docs/architecture.md, 4.13).
//!
//! No address on such a bridge does not yet keep the NAS off it: Linux
//! answers ARP for any of its addresses on every interface (`arp_ignore`
//! 0) and takes in what comes on one for any of its addresses, so a VM on
//! the bridge could reach the NAS's LAN address through it, and with IP
//! forwarding on (Docker switches it on) be routed further. So a table of
//! zendiloOS's in nftables' `bridge` family drops every frame such a bridge
//! hands up to the NAS (`input`) and every frame the NAS would send out on
//! it (`output`). Frames between the VMs' ports are forwarded by the bridge
//! itself and meet neither hook; with `br_netfilter` loaded they also pass
//! the IP firewall's forward chains with the bridge as both interfaces,
//! where zendiloOS's firewall lets them through and Docker's FORWARD chain
//! does through `ZENDILOOS-BRIDGES` ([`crate::docker_bridges`]), which
//! lists every bridge zendiloOS made.
//!
//! The rules go to `/etc/zendiloos/network/internal-networks.nft`, which
//! replaces the table `bridge zendiloos_internal` in one transaction (and
//! only removes it while there is no internal network);
//! `zendiloos-internal-networks.service` loads it at boot, before the
//! network comes up, and again whenever Debian's `nftables.service` flushed
//! everything. The agent writes it and has the unit load it after every
//! change of the network's devices and its undoing, and as it starts. A
//! card goes on an internal network only while the rules in force list it.

use std::fs;
use std::path::Path;
use std::time::Duration;

use zendiloos_agent_proto::InterfaceName;
use zendiloos_exec::{Limits, Program};

use crate::network::PREFIX;
use crate::operations::OperationError;

/// Loads the rules: at boot, after Debian's nftables.service, and when the
/// agent asks.
pub(crate) const UNIT: &str = "zendiloos-internal-networks.service";
const HEADER: &str = "# Written by zendiloOS: what the NAS keeps off its internal networks for VMs.\n\
                      # Changes made here are replaced.\n";
/// Creating the table first makes deleting it work whether or not it is
/// there; all in one transaction with what follows.
const REPLACE: &str = "table bridge zendiloos_internal\ndelete table bridge zendiloos_internal\n";
const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };

/// The `Key=value` lines of a systemd-networkd file, trimmed, without
/// comments.
fn settings(text: &str) -> Vec<(&str, &str)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.starts_with(';'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect()
}

/// The bridges that are internal networks, by name: each a `.netdev` of
/// zendiloOS's of the kind `bridge`, no file of zendiloOS's making an
/// interface its port, and its own `.network` file giving it no address:
/// neither DHCP nor a fixed address, neither router advertisements nor a
/// link-local address. A bridge with a card, or with an address of the
/// NAS's, is the LAN's or the NAS's and none of these.
pub(crate) fn internal_bridges(directory: &Path) -> Vec<InterfaceName> {
    let Ok(entries) = fs::read_dir(directory) else { return Vec::new() };
    let mut netdevs = Vec::new();
    let mut networks = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(PREFIX) else { continue };
        let Ok(text) = fs::read_to_string(entry.path()) else { continue };
        if let Some(device) = rest.strip_suffix(".netdev") {
            netdevs.push((device.to_owned(), text));
        } else if let Some(interface) = rest.strip_suffix(".network") {
            networks.push((interface.to_owned(), text));
        }
    }
    let mut bridges: Vec<InterfaceName> = netdevs
        .iter()
        .filter_map(|(device, text)| {
            let device = InterfaceName::parse(device).ok()?;
            let lines = settings(text);
            let bridge = lines.contains(&("Kind", "bridge"));
            let named = lines.contains(&("Name", device.as_str()));
            (bridge && named).then_some(device)
        })
        .filter(|bridge| {
            let ported =
                networks.iter().any(|(_, text)| settings(text).contains(&("Bridge", bridge.as_str())));
            let own = networks.iter().find(|(interface, _)| interface == bridge.as_str());
            let addressless = own.is_some_and(|(_, text)| {
                let lines = settings(text);
                let says = |key: &str, value: &str| lines.contains(&(key, value));
                says("DHCP", "no")
                    && says("IPv6AcceptRA", "no")
                    && says("LinkLocalAddressing", "no")
                    && !lines.iter().any(|(key, _)| matches!(*key, "Address" | "Gateway" | "Bridge" | "Bond"))
            });
            !ported && addressless
        })
        .collect();
    bridges.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    bridges
}

/// The rules file: the table replaced by drops of what goes between the NAS
/// and `bridges`, or removed while there is none.
pub(crate) fn ruleset(bridges: &[InterfaceName]) -> String {
    let mut text = format!("{HEADER}{REPLACE}");
    if bridges.is_empty() {
        return text;
    }
    let names: Vec<String> = bridges.iter().map(|bridge| format!("\"{}\"", bridge.as_str())).collect();
    let names = names.join(", ");
    text.push_str(&format!(
        "table bridge zendiloos_internal {{\n\
         \tchain input {{\n\
         \t\ttype filter hook input priority filter; policy accept;\n\
         \t\t# Nothing on an internal network reaches the NAS.\n\
         \t\tmeta ibrname {{ {names} }} drop\n\
         \t}}\n\
         \tchain output {{\n\
         \t\ttype filter hook output priority filter; policy accept;\n\
         \t\t# Nor does the NAS send anything onto one.\n\
         \t\tmeta obrname {{ {names} }} drop\n\
         \t}}\n\
         }}\n"
    ));
    text
}

/// Writes the rules for the internal networks the files in `directory`
/// make to `file` and has the unit load them.
pub(crate) async fn sync(systemctl: &Program, directory: &Path, file: &Path) -> Result<(), OperationError> {
    let text = ruleset(&internal_bridges(directory));
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
    }
    crate::durable::replace(file, text.as_bytes())?;
    zendiloos_exec::run(systemctl, ["reload-or-restart", UNIT], LIMITS).await?.success(systemctl)?;
    Ok(())
}

/// Whether the NAS keeps itself off `bridge` now: it is an internal
/// network by the files in `directory`, the rules in `file` are those for
/// them, and the unit that loaded them is active.
pub(crate) async fn keeps_off(
    systemctl: &Program,
    directory: &Path,
    file: &Path,
    bridge: &InterfaceName,
) -> bool {
    let bridges = internal_bridges(directory);
    if !bridges.contains(bridge) || fs::read_to_string(file).ok() != Some(ruleset(&bridges)) {
        return false;
    }
    zendiloos_exec::run(systemctl, ["is-active", "--quiet", UNIT], LIMITS)
        .await
        .is_ok_and(|output| output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> InterfaceName {
        InterfaceName::parse(text).unwrap()
    }

    const OFF: &str = "[Match]\nName=vmnet0\n\n[Link]\nRequiredForOnline=no\n\n\
                       [Network]\nDHCP=no\nIPv6AcceptRA=no\nLinkLocalAddressing=no\n";

    #[test]
    fn internal_networks_are_bridges_without_members_or_an_address() {
        let folder = tempfile::tempdir().unwrap();
        let write = |file: &str, text: &str| fs::write(folder.path().join(file), text).unwrap();
        // Internal: made by zendiloOS, no port, no address.
        write("05-zendiloos-vmnet0.netdev", "[NetDev]\nName=vmnet0\nKind=bridge\n\n[Bridge]\nSTP=no\n");
        write("05-zendiloos-vmnet0.network", OFF);
        // The LAN's: a card is its port.
        write("05-zendiloos-br0.netdev", "# Members: enp1s0\n[NetDev]\nName=br0\nKind=bridge\n");
        write("05-zendiloos-br0.network", &OFF.replace("vmnet0", "br0").replace("DHCP=no", "DHCP=ipv4"));
        write("05-zendiloos-enp1s0.network", "[Match]\nName=enp1s0\n\n[Network]\nBridge=br0\nDHCP=no\n");
        // The NAS has an address on it: VMs reach the NAS there.
        write("05-zendiloos-host0.netdev", "[NetDev]\nName=host0\nKind=bridge\n");
        write(
            "05-zendiloos-host0.network",
            &OFF.replace("vmnet0", "host0")
                .replace("LinkLocalAddressing=no\n", "LinkLocalAddressing=no\nAddress=10.9.0.1/24\n"),
        );
        // Router advertisements taken: an address may come.
        write("05-zendiloos-ra0.netdev", "[NetDev]\nName=ra0\nKind=bridge\n");
        write(
            "05-zendiloos-ra0.network",
            &OFF.replace("vmnet0", "ra0").replace("IPv6AcceptRA=no", "IPv6AcceptRA=yes"),
        );
        // A bridge without a file of its own: the kernel would give it a link-local address.
        write("05-zendiloos-bare.netdev", "[NetDev]\nName=bare\nKind=bridge\n");
        // A bond, and a bridge not of zendiloOS's.
        write("05-zendiloos-bond0.netdev", "[NetDev]\nName=bond0\nKind=bond\n");
        write("05-zendiloos-bond0.network", &OFF.replace("vmnet0", "bond0"));
        write("10-mine.netdev", "[NetDev]\nName=mine\nKind=bridge\n");
        write("10-mine.network", &OFF.replace("vmnet0", "mine"));
        assert_eq!(internal_bridges(folder.path()), [name("vmnet0")]);
        assert!(internal_bridges(&folder.path().join("missing")).is_empty());
    }

    #[test]
    fn the_nas_drops_what_goes_between_it_and_them() {
        let text = ruleset(&[name("vmnet0"), name("lab")]);
        assert!(text.starts_with("# Written by zendiloOS"));
        assert!(text.contains("table bridge zendiloos_internal\ndelete table bridge zendiloos_internal\n"));
        assert!(text.contains("type filter hook input priority filter; policy accept;\n"));
        assert!(text.contains("\t\tmeta ibrname { \"vmnet0\", \"lab\" } drop\n"), "{text}");
        assert!(text.contains("\t\tmeta obrname { \"vmnet0\", \"lab\" } drop\n"), "{text}");
        assert!(!text.contains("forward"), "frames between the VMs are the bridge's: {text}");
        // None: the table goes.
        assert_eq!(ruleset(&[]), format!("{HEADER}{REPLACE}"));
    }

    #[tokio::test]
    async fn written_loaded_and_kept_only_while_in_force() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir().unwrap();
        let network = folder.path().join("network");
        fs::create_dir_all(&network).unwrap();
        fs::write(network.join("05-zendiloos-vmnet0.netdev"), "[NetDev]\nName=vmnet0\nKind=bridge\n")
            .unwrap();
        fs::write(network.join("05-zendiloos-vmnet0.network"), OFF).unwrap();
        let script = folder.path().join("systemctl");
        let log = folder.path().join("calls");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$*\" >> {}\n[ \"$1\" != is-active ] || [ -e {}/active ]\n",
                log.display(),
                folder.path().display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let systemctl = Program::new(&script).unwrap();
        let file = folder.path().join("zendiloos/internal-networks.nft");

        assert!(!keeps_off(&systemctl, &network, &file, &name("vmnet0")).await, "not written yet");
        sync(&systemctl, &network, &file).await.unwrap();
        assert!(fs::read_to_string(&file).unwrap().contains("meta ibrname { \"vmnet0\" } drop"));
        assert!(
            fs::read_to_string(&log)
                .unwrap()
                .contains("reload-or-restart zendiloos-internal-networks.service")
        );
        assert!(!keeps_off(&systemctl, &network, &file, &name("vmnet0")).await, "the unit failed");
        fs::write(folder.path().join("active"), "").unwrap();
        assert!(keeps_off(&systemctl, &network, &file, &name("vmnet0")).await);
        assert!(!keeps_off(&systemctl, &network, &file, &name("br0")).await, "no internal network");
        // A card added since: no longer internal, nor kept off.
        fs::write(network.join("05-zendiloos-enp2s0.network"), "[Network]\nBridge=vmnet0\n").unwrap();
        assert!(!keeps_off(&systemctl, &network, &file, &name("vmnet0")).await);
    }
}
