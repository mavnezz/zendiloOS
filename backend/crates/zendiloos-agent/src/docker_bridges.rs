//! Frames within the bridges zendiloOS made, let through Docker's FORWARD
//! chain: the one place zendiloOS touches Docker's tables.
//!
//! Docker (Debian 13's docker.io is 26.1) loads `br_netfilter` as it
//! starts, which sends frames a bridge forwards between its ports through
//! the IP firewall's forward hooks (`bridge-nf-call-iptables`, 1 once the
//! module is there), and sets the policy of iptables' FORWARD chain to DROP
//! when it has to switch IP forwarding on, as on every NAS that does not
//! route. A frame from a VM's tap to the card on the same bridge then ends
//! in that policy; an accept in zendiloOS's own nftables table cannot help,
//! since every table's verdict counts. Docker keeps the chain DOCKER-USER
//! for others' rules, jumps to it first from FORWARD and never changes
//! what is in it; its documentation suggests `-i <if> -o <if> -j ACCEPT`
//! there to let forwarding between the host's interfaces pass.
//!
//! So `zendiloos-docker-bridges.service` (this binary as `docker-bridges`)
//! keeps one jump in DOCKER-USER to a chain of zendiloOS's,
//! `ZENDILOOS-BRIDGES`, which accepts what comes in on one of its bridges
//! and goes out on the same, for IPv4 and IPv6 alike, and nothing else:
//! frames between two bridges, or from a bridge to Docker's containers,
//! still meet Docker's rules. With no bridge left, the jump and the chain
//! go. It runs after every start of Docker (it is wanted by and part of
//! `docker.service`), and the agent starts it after every change of the
//! bridges and as it starts. The bridges are those of zendiloOS's
//! `.netdev` files: a bridge made otherwise is its owner's to let through.
//!
//! It runs `iptables` as Debian chose it (`iptables-nft` by default), the
//! same Docker runs, so both write the same tables in the same way.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use zendiloos_agent_proto::InterfaceName;
use zendiloos_exec::{Limits, Program};

use crate::network::PREFIX;

/// zendiloOS's chain in the filter table.
pub const CHAIN: &str = "ZENDILOOS-BRIDGES";
/// Docker's chain for others' rules.
const DOCKER_USER: &str = "DOCKER-USER";
const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 1024 * 1024 };

/// One address family's tools: `iptables` and `iptables-restore`, or
/// `ip6tables` and `ip6tables-restore`.
#[derive(Debug, Clone)]
pub struct Family {
    pub name: &'static str,
    pub tables: PathBuf,
    pub restore: PathBuf,
}

impl Family {
    /// Debian's, through its alternatives.
    pub fn debian() -> [Self; 2] {
        [
            Self {
                name: "iptables",
                tables: "/usr/sbin/iptables".into(),
                restore: "/usr/sbin/iptables-restore".into(),
            },
            Self {
                name: "ip6tables",
                tables: "/usr/sbin/ip6tables".into(),
                restore: "/usr/sbin/ip6tables-restore".into(),
            },
        ]
    }
}

/// The bridges zendiloOS made: its `.netdev` files of the kind `bridge`,
/// each name checked again, by name.
pub fn own_bridges(directory: &Path) -> Vec<InterfaceName> {
    let Ok(entries) = fs::read_dir(directory) else { return Vec::new() };
    let mut bridges: Vec<InterfaceName> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let device = name.strip_prefix(PREFIX)?.strip_suffix(".netdev")?;
            let device = InterfaceName::parse(device).ok()?;
            let text = fs::read_to_string(entry.path()).ok()?;
            let bridge = text.lines().any(|line| line.trim() == "Kind=bridge");
            let named = text.lines().any(|line| line.trim().strip_prefix("Name=") == Some(device.as_str()));
            (bridge && named).then_some(device)
        })
        .collect();
    bridges.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    bridges
}

/// What `iptables-restore --noflush` reads: the chain made if missing and
/// filled anew, in one transaction.
pub fn chain_rules(bridges: &[InterfaceName]) -> String {
    let mut text = format!("*filter\n:{CHAIN} - [0:0]\n-F {CHAIN}\n");
    for bridge in bridges {
        let name = bridge.as_str();
        text.push_str(&format!("-A {CHAIN} -i {name} -o {name} -j ACCEPT\n"));
    }
    text.push_str("COMMIT\n");
    text
}

async fn succeeds(program: &Program, args: &[&str]) -> Result<bool, String> {
    let output = zendiloos_exec::run(program, args, LIMITS).await.map_err(|e| e.to_string())?;
    Ok(output.status.success())
}

async fn must(program: &Program, args: &[&str]) -> Result<(), String> {
    let output = zendiloos_exec::run(program, args, LIMITS).await.map_err(|e| e.to_string())?;
    output.success(program).map(|_| ()).map_err(|e| e.to_string())
}

/// Brings one family's tables up to date with `bridges`; what it did, in
/// a line.
pub async fn sync_family(family: &Family, bridges: &[InterfaceName]) -> Result<String, String> {
    if !family.tables.exists() {
        return Ok(format!("{}: not installed, nothing to do", family.name));
    }
    let tables = Program::new(&family.tables).map_err(|e| e.to_string())?;
    // `-w`: Docker may be changing the tables at the same time.
    let wait = ["-w", "10", "-t", "filter"];
    let with = |rest: &[&'static str]| -> Vec<&'static str> { [&wait[..], rest].concat() };
    let docker = succeeds(&tables, &with(&["-L", DOCKER_USER, "-n"])).await?;
    let ours = succeeds(&tables, &with(&["-L", CHAIN, "-n"])).await?;
    let check = with(&["-C", DOCKER_USER, "-j", CHAIN]);
    if bridges.is_empty() || !docker {
        // Nothing to let through, or no Docker: nothing of zendiloOS's in
        // its tables.
        if docker {
            let delete = with(&["-D", DOCKER_USER, "-j", CHAIN]);
            // Once, but every copy should there be several.
            for _ in 0..8 {
                if !succeeds(&tables, &check).await? {
                    break;
                }
                must(&tables, &delete).await?;
            }
        }
        if ours {
            must(&tables, &with(&["-F", CHAIN])).await?;
            must(&tables, &with(&["-X", CHAIN])).await?;
        }
        return Ok(if docker {
            format!("{}: no bridge of zendiloOS's, nothing in Docker's chain", family.name)
        } else {
            format!("{}: Docker's chain {DOCKER_USER} is not there, nothing to do", family.name)
        });
    }
    let restore = Program::new(&family.restore).map_err(|e| e.to_string())?;
    let rules = chain_rules(bridges);
    let output =
        zendiloos_exec::run_with_input(&restore, ["-w", "10", "--noflush"], rules.as_bytes(), LIMITS)
            .await
            .map_err(|e| e.to_string())?;
    output.success(&restore).map_err(|e| e.to_string())?;
    if !succeeds(&tables, &check).await? {
        // First in DOCKER-USER, before the RETURN Docker puts there.
        must(&tables, &with(&["-I", DOCKER_USER, "1", "-j", CHAIN])).await?;
    }
    let names: Vec<&str> = bridges.iter().map(InterfaceName::as_str).collect();
    Ok(format!("{}: frames within {} pass Docker's FORWARD chain", family.name, names.join(", ")))
}

/// `docker-bridges`: both families brought up to date with the bridges in
/// `directory`; a line for each, or why one failed.
pub async fn sync(directory: &Path, families: &[Family]) -> Result<Vec<String>, String> {
    let bridges = own_bridges(directory);
    let mut report = Vec::new();
    let mut failed = Vec::new();
    for family in families {
        match sync_family(family, &bridges).await {
            Ok(line) => report.push(line),
            Err(error) => failed.push(format!("{}: {error}", family.name)),
        }
    }
    if failed.is_empty() { Ok(report) } else { Err(failed.join("; ")) }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn name(text: &str) -> InterfaceName {
        InterfaceName::parse(text).unwrap()
    }

    #[test]
    fn only_zendiloos_bridges_count() {
        let folder = tempfile::tempdir().unwrap();
        let write = |file: &str, text: &str| fs::write(folder.path().join(file), text).unwrap();
        write("05-zendiloos-br0.netdev", "# Members: enp1s0\n[NetDev]\nName=br0\nKind=bridge\n");
        write("05-zendiloos-vm.netdev", "[NetDev]\nName=vm\nKind=bridge\n");
        write("05-zendiloos-bond0.netdev", "[NetDev]\nName=bond0\nKind=bond\n");
        write("05-zendiloos-odd.netdev", "[NetDev]\nName=other\nKind=bridge\n");
        write("10-mine.netdev", "[NetDev]\nName=br9\nKind=bridge\n");
        write("05-zendiloos-br0.network", "[Network]\nKind=bridge\n");
        assert_eq!(own_bridges(folder.path()), [name("br0"), name("vm")]);
        assert!(own_bridges(&folder.path().join("missing")).is_empty());
    }

    #[test]
    fn the_chain_is_made_and_filled_in_one_go() {
        assert_eq!(
            chain_rules(&[name("br0"), name("vm")]),
            "*filter\n:ZENDILOOS-BRIDGES - [0:0]\n-F ZENDILOOS-BRIDGES\n\
             -A ZENDILOOS-BRIDGES -i br0 -o br0 -j ACCEPT\n-A ZENDILOOS-BRIDGES -i vm -o vm -j ACCEPT\nCOMMIT\n"
        );
    }

    /// `iptables` as a script with the tables as files: DOCKER-USER while
    /// `docker-user` is there, the jump as a line in it, the chain while
    /// `chain` is there.
    fn fake(folder: &Path) -> Family {
        let script = |name: &str, body: &str| {
            let path = folder.join(name);
            fs::write(
                &path,
                format!(
                    "#!/bin/sh\necho \"{name} $*\" >> {}/calls\nR={}\n{body}\n",
                    folder.display(),
                    folder.display()
                ),
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let tables = script(
            "iptables",
            "shift 4\n\
             case \"$1 $2\" in\n\
             \"-L DOCKER-USER\") [ -e $R/docker-user ] ;;\n\
             \"-L ZENDILOOS-BRIDGES\") [ -e $R/chain ] ;;\n\
             \"-C DOCKER-USER\") grep -q jump $R/docker-user 2>/dev/null ;;\n\
             \"-I DOCKER-USER\") echo jump >> $R/docker-user ;;\n\
             \"-D DOCKER-USER\") sed -i '0,/jump/{/jump/d}' $R/docker-user ;;\n\
             \"-F ZENDILOOS-BRIDGES\") : > $R/chain ;;\n\
             \"-X ZENDILOOS-BRIDGES\") rm $R/chain ;;\n\
             *) exit 2 ;;\n\
             esac",
        );
        let restore = script("iptables-restore", "cat > $R/chain");
        Family { name: "iptables", tables, restore }
    }

    #[tokio::test]
    async fn one_jump_to_a_chain_of_its_own_and_gone_with_the_last_bridge() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        let family = fake(root);

        // No Docker: nothing.
        let line = sync_family(&family, &[name("br0")]).await.unwrap();
        assert!(line.contains("not there"), "{line}");
        assert!(!root.join("chain").exists());

        fs::write(root.join("docker-user"), "return\n").unwrap();
        sync_family(&family, &[name("br0")]).await.unwrap();
        let line = sync_family(&family, &[name("br0"), name("vm")]).await.unwrap();
        assert_eq!(line, "iptables: frames within br0, vm pass Docker's FORWARD chain");
        assert_eq!(fs::read_to_string(root.join("docker-user")).unwrap(), "return\njump\n", "one jump");
        assert!(fs::read_to_string(root.join("chain")).unwrap().contains("-i vm -o vm -j ACCEPT"));
        let calls = fs::read_to_string(root.join("calls")).unwrap();
        assert!(calls.contains("iptables-restore -w 10 --noflush"), "{calls}");
        assert!(calls.contains("iptables -w 10 -t filter -I DOCKER-USER 1 -j ZENDILOOS-BRIDGES"), "{calls}");

        // The last bridge gone: Docker's chain as Docker made it.
        let line = sync_family(&family, &[]).await.unwrap();
        assert!(line.contains("no bridge"), "{line}");
        assert_eq!(fs::read_to_string(root.join("docker-user")).unwrap(), "return\n");
        assert!(!root.join("chain").exists());

        // Not installed: nothing either.
        let missing = Family { tables: root.join("none"), ..family };
        assert!(sync_family(&missing, &[name("br0")]).await.unwrap().contains("not installed"));
    }
}
