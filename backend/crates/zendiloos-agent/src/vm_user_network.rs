//! What a VM's cards through passt reach (docs/architecture.md, 4.13).
//!
//! passt makes every connection of the guest from a socket of its own on
//! the host, as QEMU's account (`libvirt-qemu`), which libvirt runs it as.
//! Left alone, the guest reaches whatever the NAS itself reaches: the
//! containers' networks on Docker's bridges (the host routes there, and
//! what the host sends never meets the forward chains), the NAS's own
//! services past the firewall (local traffic comes in on the loopback,
//! which the firewall trusts), and, as passt maps the guest's gateway to
//! the host's loopback, what listens on 127.0.0.1 alone, the web interface
//! then seeing the guest as the machine itself.
//!
//! The gateway's mapping is also the only way the guest reaches the NAS:
//! passt gives it the NAS's own address, so the NAS's address is the
//! guest's own in there. So the loopback is not closed, but held to what
//! the LAN gets. A table of zendiloOS's in nftables' `inet` family, `inet
//! zendiloos_vms`, looks at what QEMU's account sends, before the
//! firewall's chains (priority `filter - 10`): DNS to the loopback passes
//! (passt hands the guest's queries to the host's resolver, which is often
//! on 127.0.0.53); IPv6's loopback is refused; everything else is marked
//! in conntrack ([`CT_MARK`]). What it sends to the loopback leaves from
//! the NAS's own address (`masquerade`, which for the loopback takes the
//! first address of the machine's cards, the LAN's), not as the NAS
//! itself, so the web interface sees no 127.0.0.1, which its blocking of
//! addresses never blocks; and arriving it reaches only a socket that
//! listens on every address (`socket wildcard`), as the LAN would, never
//! one on the loopback alone (NUT's server, a port published to
//! 127.0.0.1). By the mark the firewall's `input` chain sends what reaches
//! the NAS through its rules as it does the LAN's instead of trusting the
//! loopback, and its `output` chain what goes to a container's published
//! port (4.12). A connection Docker sends on to a published port is marked
//! on its packets too ([`PUBLISHED_MARK`]): a table of the `bridge`
//! family, `bridge zendiloos_vms`, drops what QEMU's account sends out of
//! a bridge's port that is a container's (`veth…`) without that mark, so
//! no container is reached on its own address, whatever its bridge is
//! called. A port published to the loopback alone stays the loopback's
//! where Docker runs without its proxy and rewrites the connection itself.
//!
//! Anywhere else a connection leaves only by the cards of the host's
//! default routes, as a LAN client gets there through its router: none
//! goes into another network the NAS reaches, a tunnel (WireGuard,
//! Tailscale) or a second card on a storage or management network, which
//! the LAN cannot reach through the NAS (it routes nothing for it). The
//! cards are read from the host's routes (`/proc/1/net/route`,
//! `ipv6_route`: the agent's own network is another) and named in the
//! rules; while there is no default route (at boot, before DHCP), the
//! cards in force stay. The agent looks again every half minute and
//! writes the rules anew when the cards change.
//!
//! The rules go to `/etc/zendiloos/network/passt.nft`, which
//! replaces both tables in one transaction (and only removes them while
//! there is no QEMU account); `zendiloos-passt.service` loads it
//! at boot, before the network and libvirt, and again whenever Debian's
//! `nftables.service` flushed everything. The agent writes it as the VMs
//! are set up and as it starts, and starts a VM with a card through passt
//! only while the rules in force are these.

use std::fs;
use std::path::Path;
use std::time::Duration;

use zendiloos_exec::{Limits, Program};

use crate::operations::OperationError;

/// The rules' file.
pub(crate) const FILE: &str = "/etc/zendiloos/network/passt.nft";
/// Loads the rules: at boot, after Debian's nftables.service, and when the
/// agent asks.
pub(crate) const UNIT: &str = "zendiloos-passt.service";
/// The account QEMU and passt run as, Debian's for libvirt.
pub(crate) const QEMU_USER: &str = "libvirt-qemu";
/// The bit of a connection's conntrack mark that says passt made it for a
/// VM; the firewall's rules decide what of the NAS it reaches.
pub(crate) const CT_MARK: u32 = 0x0020_0000;
/// The bit of a packet's mark that says it is one of passt's to a
/// container's published port, which may go on to the container.
pub(crate) const PUBLISHED_MARK: u32 = 0x0040_0000;
const HEADER: &str =
    "# Written by zendiloOS: what VMs reach through passt. Changes made here are replaced.\n";
/// The line naming the cards passt's connections may leave by, so that
/// they are kept while the host has no default route (at boot).
const UPLINKS: &str = "# Out by: ";
/// Creating the tables first makes deleting them work whether or not they
/// are there; all in one transaction with what follows.
const REPLACE: &str = "table inet zendiloos_vms\ndelete table inet zendiloos_vms\n\
                       table bridge zendiloos_vms\ndelete table bridge zendiloos_vms\n";
const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };

/// The rules for what the account `uid` sends, which leaves the NAS only
/// by `uplinks` (the cards of its default routes); only the tables'
/// removal without the account. Without any uplink known, where it leaves
/// is not held.
pub(crate) fn ruleset(uid: Option<u32>, uplinks: &[String]) -> String {
    let mut text = format!("{HEADER}{REPLACE}");
    let Some(uid) = uid else { return text };
    let elsewhere = if uplinks.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = uplinks.iter().map(|name| format!("\"{name}\"")).collect();
        format!(
            "\t\t# Elsewhere only as the LAN gets there: out by the cards of the\n\
             \t\t# default routes, into no other network the NAS reaches (a\n\
             \t\t# tunnel, a card on a storage network).\n\
             \t\toifname != {{ {} }} reject with icmpx admin-prohibited\n",
            names.join(", ")
        )
    };
    text.push_str(&format!(
        "{UPLINKS}{}\n\
         table inet zendiloos_vms {{\n\
         \tchain output {{\n\
         \t\ttype filter hook output priority filter - 10; policy accept;\n\
         \t\t# passt, which makes the connections of VMs' cards as QEMU's account.\n\
         \t\tmeta skuid {uid} jump passt\n\
         \t}}\n\
         \tchain passt {{\n\
         \t\t# The host's resolver, which passt hands the guest's DNS to.\n\
         \t\tip daddr 127.0.0.0/8 meta l4proto {{ tcp, udp }} th dport 53 accept\n\
         \t\tip6 daddr ::1 meta l4proto {{ tcp, udp }} th dport 53 accept\n\
         \t\t# The NAS through IPv6's loopback would be the NAS itself: nothing.\n\
         \t\tip6 daddr ::1 reject with icmpx admin-prohibited\n\
         \t\t# A port published to the loopback alone, which Docker sends on itself\n\
         \t\t# where it runs without its proxy: the loopback's, not the LAN's.\n\
         \t\tct original ip daddr 127.0.0.0/8 ct status dnat reject with icmpx admin-prohibited\n\
         \t\t# The firewall's rules decide what of the NAS it reaches, as for the LAN.\n\
         \t\tct mark set ct mark or {CT_MARK:#010x}\n\
         \t\t# A container's published port: Docker sends it on to the container.\n\
         \t\tct status dnat meta mark set meta mark or {PUBLISHED_MARK:#010x} accept\n\
         \t\t# The NAS itself, through the loopback (below).\n\
         \t\toifname \"lo\" accept\n\
         {elsewhere}\
         \t}}\n\
         \tchain input {{\n\
         \t\ttype filter hook input priority filter - 10; policy accept;\n\
         \t\t# passt maps the guest's gateway to the loopback: through it, only\n\
         \t\t# what listens on every address, as the LAN reaches it, never what\n\
         \t\t# listens on the loopback alone.\n\
         \t\tct mark and {CT_MARK:#010x} != 0 ct state new ip daddr 127.0.0.0/8 socket wildcard 0 \
         reject with icmpx admin-prohibited\n\
         \t}}\n\
         \tchain postrouting {{\n\
         \t\ttype nat hook postrouting priority srcnat; policy accept;\n\
         \t\t# Not from 127.0.0.1, as if it were the NAS itself: from the NAS's\n\
         \t\t# own address, as its other connections leave.\n\
         \t\tmeta skuid {uid} ip daddr 127.0.0.0/8 meta l4proto {{ tcp, udp }} th dport != 53 masquerade\n\
         \t\tmeta skuid {uid} ip daddr 127.0.0.0/8 meta l4proto icmp masquerade\n\
         \t}}\n\
         }}\n\
         table bridge zendiloos_vms {{\n\
         \tchain output {{\n\
         \t\ttype filter hook output priority filter; policy accept;\n\
         \t\t# No container on its own address: only through a published port.\n\
         \t\tmeta skuid {uid} oifname \"veth*\" meta mark and {PUBLISHED_MARK:#010x} == 0 drop\n\
         \t}}\n\
         }}\n",
        uplinks.join(" ")
    ));
    text
}

/// Where the rules come from and go: the file, the accounts (QEMU's), and
/// the host's routes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Places<'a> {
    pub file: &'a Path,
    pub passwd: &'a Path,
    /// `/proc/1/net/route`.
    pub routes: &'a Path,
    /// `/proc/1/net/ipv6_route`.
    pub routes6: &'a Path,
}

/// The uid of QEMU's account in `passwd`; none if there is none.
fn qemu_uid(passwd: &Path) -> Option<u32> {
    let text = fs::read_to_string(passwd).ok()?;
    crate::passwd::lookup(&text, QEMU_USER).map(|(uid, _)| uid)
}

/// The cards the host's default routes leave by, IPv4's
/// (`/proc/net/route`: destination and mask 0) and IPv6's
/// (`/proc/net/ipv6_route`: `::/0`, up, not a reject route), by name.
fn default_route_cards(routes: &Path, routes6: &Path) -> Vec<String> {
    const RTF_UP: u32 = 0x0001;
    const RTF_REJECT: u32 = 0x0200;
    let mut cards = Vec::new();
    let flags = |text: &str| u32::from_str_radix(text, 16).unwrap_or(0);
    let name_ok = |name: &str| zendiloos_agent_proto::InterfaceName::parse(name).is_ok() && name != "lo";
    if let Ok(text) = fs::read_to_string(routes) {
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if let [card, destination, _, flag, _, _, _, mask, ..] = fields[..]
                && destination == "00000000"
                && mask == "00000000"
                && flags(flag) & RTF_UP != 0
                && name_ok(card)
            {
                cards.push(card.to_owned());
            }
        }
    }
    if let Ok(text) = fs::read_to_string(routes6) {
        for line in text.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if let [destination, prefix, .., flag, card] = fields[..]
                && fields.len() == 10
                && destination.bytes().all(|b| b == b'0')
                && prefix == "00"
                && flags(flag) & RTF_UP != 0
                && flags(flag) & RTF_REJECT == 0
                && name_ok(card)
            {
                cards.push(card.to_owned());
            }
        }
    }
    cards.sort();
    cards.dedup();
    cards
}

/// The cards the rules in `file` let passt's connections leave by.
fn uplinks_in(file: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(file) else { return Vec::new() };
    text.lines()
        .find_map(|line| line.strip_prefix(UPLINKS))
        .map(|names| {
            names
                .split_whitespace()
                .filter(|name| zendiloos_agent_proto::InterfaceName::parse(name).is_ok())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The cards passt's connections may leave by: those of the default routes
/// now; while there is none (the network not up yet at boot), those the
/// rules in force had.
fn uplinks(places: &Places<'_>) -> Vec<String> {
    let now = default_route_cards(places.routes, places.routes6);
    if now.is_empty() { uplinks_in(places.file) } else { now }
}

/// Whether the file holds the rules there are to be now; it may be put in
/// force without asking systemd.
pub(crate) fn written(places: &Places<'_>) -> bool {
    let Some(uid) = qemu_uid(places.passwd) else { return true };
    fs::read_to_string(places.file).ok() == Some(ruleset(Some(uid), &uplinks(places)))
}

/// Writes the rules for QEMU's account (by `passwd`) and the default
/// routes' cards to the file and has the unit load them.
pub(crate) async fn sync(systemctl: &Program, places: &Places<'_>) -> Result<(), OperationError> {
    let text = ruleset(qemu_uid(places.passwd), &uplinks(places));
    let file = places.file;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
    }
    if fs::read_to_string(file).ok().as_deref() != Some(text.as_str()) {
        crate::durable::replace(file, text.as_bytes())?;
    }
    zendiloos_exec::run(systemctl, ["reload-or-restart", UNIT], LIMITS).await?.success(systemctl)?;
    Ok(())
}

/// Whether the rules for QEMU's account are in force: the file holds them,
/// for the default routes' cards there are now, and the unit that loaded
/// it is active. Without the account nothing runs as it, and nothing is to
/// be kept off.
pub(crate) async fn in_force(systemctl: &Program, places: &Places<'_>) -> bool {
    let Some(uid) = qemu_uid(places.passwd) else { return true };
    if fs::read_to_string(places.file).ok() != Some(ruleset(Some(uid), &uplinks(places))) {
        return false;
    }
    zendiloos_exec::run(systemctl, ["is-active", "--quiet", UNIT], LIMITS)
        .await
        .is_ok_and(|output| output.status.success())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn cards(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn what_qemus_account_sends_is_held_to_the_lan_and_the_world() {
        let text = ruleset(Some(64055), &cards(&["br0", "enp2s0"]));
        assert!(text.starts_with(HEADER));
        assert!(text.contains(REPLACE));
        assert!(text.contains("\t\tmeta skuid 64055 jump passt\n"));
        assert!(text.contains("priority filter - 10;"), "before the firewall's chains");
        // DNS to the loopback as it is, unmarked; IPv6's loopback not at all.
        let at = |line: &str| text.find(line).unwrap_or_else(|| panic!("no {line:?}"));
        let dns = at("ip daddr 127.0.0.0/8 meta l4proto { tcp, udp } th dport 53 accept");
        let mark = at("ct mark set ct mark or 0x00200000");
        assert!(dns < mark && at("ip6 daddr ::1 reject with icmpx admin-prohibited") < mark);
        // A port published to the loopback alone stays the loopback's, also
        // where Docker sends it on without its proxy.
        assert!(at("ct original ip daddr 127.0.0.0/8 ct status dnat reject") < mark);
        let published = at("ct status dnat meta mark set meta mark or 0x00400000 accept");
        let local = at("oifname \"lo\" accept");
        // Elsewhere only out by the default routes' cards.
        let out = at("oifname != { \"br0\", \"enp2s0\" } reject with icmpx admin-prohibited");
        assert!(mark < published && published < local && local < out);
        assert!(text.contains("# Out by: br0 enp2s0\n"));
        // Through the gateway only to what listens on every address, from
        // the NAS's own address.
        at("ct mark and 0x00200000 != 0 ct state new ip daddr 127.0.0.0/8 socket wildcard 0 reject");
        at("type nat hook postrouting priority srcnat;");
        at("meta skuid 64055 ip daddr 127.0.0.0/8 meta l4proto { tcp, udp } th dport != 53 masquerade");
        at("meta skuid 64055 oifname \"veth*\" meta mark and 0x00400000 == 0 drop");
        // No default route known: where it leaves is not held.
        assert!(!ruleset(Some(64055), &[]).contains("oifname != {"));
        // Without QEMU's account the tables only go.
        assert_eq!(ruleset(None, &cards(&["br0"])), format!("{HEADER}{REPLACE}"));
    }

    #[test]
    fn the_default_routes_cards_are_read_from_the_hosts_routes() {
        let dir = tempfile::tempdir().unwrap();
        let (routes, routes6) = (dir.path().join("route"), dir.path().join("ipv6_route"));
        fs::write(
            &routes,
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
             br0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
             br0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
             wg0\t0000080A\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
             enp2s0\t0032A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n",
        )
        .unwrap();
        fs::write(
            &routes6,
            "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000003     br0\n\
             fd000000000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001     br0\n\
             00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200       lo\n\
             00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 00000400 00000001 00000000 00000001 tailscale0\n",
        )
        .unwrap();
        assert_eq!(default_route_cards(&routes, &routes6), cards(&["br0", "tailscale0"]));
        assert!(default_route_cards(&dir.path().join("none"), &dir.path().join("none")).is_empty());
    }

    fn systemctl(dir: &Path, status: u8) -> Program {
        let path = dir.join("systemctl");
        fs::write(
            &path,
            format!("#!/bin/sh\necho \"$@\" >> {}\nexit {status}\n", dir.join("calls").display()),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        Program::new(&path).unwrap()
    }

    #[tokio::test]
    async fn the_rules_are_written_for_qemus_account_and_the_default_routes_cards_and_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (file, passwd) = (root.join("network/passt.nft"), root.join("passwd"));
        let (routes, routes6) = (root.join("route"), root.join("ipv6_route"));
        let places = Places { file: &file, passwd: &passwd, routes: &routes, routes6: &routes6 };
        let active = systemctl(root, 0);
        // Without the account there is nothing to keep off.
        assert!(in_force(&active, &places).await);
        fs::write(
            &passwd,
            "root:x:0:0:root:/root:/bin/bash\nlibvirt-qemu:x:64055:108:Libvirt Qemu,,,:/var/lib/libvirt:/usr/sbin/nologin\n",
        )
        .unwrap();
        let header = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n";
        let default_by =
            |card: &str| format!("{header}{card}\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n");
        fs::write(&routes, default_by("enp1s0")).unwrap();
        assert!(!in_force(&active, &places).await, "nothing written yet");
        sync(&active, &places).await.unwrap();
        assert!(fs::read_to_string(&file).unwrap() == ruleset(Some(64055), &cards(&["enp1s0"])));
        assert!(in_force(&active, &places).await);
        let calls = fs::read_to_string(root.join("calls")).unwrap();
        assert!(calls.contains(&format!("reload-or-restart {UNIT}")), "the unit was not loaded");
        // No default route (the network not up yet): the cards in force stay.
        fs::write(&routes, header).unwrap();
        assert!(in_force(&active, &places).await);
        // Another card has it now (a bridge made over the LAN's): written anew.
        fs::write(&routes, default_by("br0")).unwrap();
        assert!(!in_force(&active, &places).await);
        sync(&active, &places).await.unwrap();
        assert!(fs::read_to_string(&file).unwrap() == ruleset(Some(64055), &cards(&["br0"])));
        // The unit not active: not in force.
        assert!(!in_force(&systemctl(root, 3), &places).await);
        // A unit that fails to load them fails the sync.
        assert!(sync(&systemctl(root, 1), &places).await.is_err());
    }
}
