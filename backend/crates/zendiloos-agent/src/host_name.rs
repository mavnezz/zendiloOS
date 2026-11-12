//! The machine's name. systemd-hostnamed sets it, which `hostnamectl` asks
//! over D-Bus: the static name in `/etc/hostname` and the kernel's. The
//! agent, in a UTS namespace of its own (`ProtectHostname`), never sets it
//! itself. `/etc/hosts` is the installer's, and only its names are changed:
//! where it named the old name (Debian's `127.0.1.1` line, or the address
//! the installer was given), it names the new one, a domain after it kept.
//!
//! What reads the name when it starts follows by a restart, if it runs:
//! Avahi (`<name>.local`), Samba (its NetBIOS name is the host name's, in
//! uppercase) and WS-Discovery (which asks Samba for it). Nothing of
//! systemd's units changes, so systemd is not reloaded.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use zendiloos_agent_proto::{HostName, Response};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;
use crate::web_server::WebServer;

const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };
/// The address Debian's installer gives the machine's own name.
const OWN_ADDRESS: &str = "127.0.1.1";
/// Far more than a host name and a hosts file take.
const MAX_FILE: u64 = 256 * 1024;
/// In this order: Avahi announces `<name>.local`, smbd registers its
/// service with Avahi under its NetBIOS name, wsdd2 asks Samba for that.
const FOLLOWERS: [&str; 3] = ["avahi-daemon.service", "smbd.service", "wsdd2.service"];

/// Where the name is kept; other than the real places in tests.
#[derive(Debug, Clone)]
pub struct HostNamePaths {
    /// `/etc/hostname`, which systemd-hostnamed writes.
    pub hostname: PathBuf,
    /// `/etc/hosts`.
    pub hosts: PathBuf,
}

impl Default for HostNamePaths {
    fn default() -> Self {
        Self { hostname: PathBuf::from("/etc/hostname"), hosts: PathBuf::from("/etc/hosts") }
    }
}

#[derive(Debug)]
pub(crate) struct HostNames {
    hostnamectl: Program,
    systemctl: Program,
    openssl: Program,
    paths: HostNamePaths,
}

impl HostNames {
    pub(crate) fn new(tools: &Tools, paths: HostNamePaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            hostnamectl: Program::new(&tools.hostnamectl)?,
            systemctl: Program::new(&tools.systemctl)?,
            openssl: Program::new(&tools.openssl)?,
            paths,
        })
    }

    pub(crate) fn state(&self) -> Result<Response, OperationError> {
        Ok(Response::HostNameState { hostname: self.read(&self.paths.hostname)?.unwrap_or_default() })
    }

    /// See [`zendiloos_agent_proto::Request::SetHostName`]. The name first:
    /// hostnamed refuses before anything else changed. A certificate that
    /// cannot be made anew leaves the one before, which is told.
    pub(crate) async fn set(&self, name: &HostName, web: &WebServer) -> Result<Response, OperationError> {
        let old = self.read(&self.paths.hostname)?.as_deref().and_then(static_name).map(str::to_owned);
        zendiloos_exec::run(&self.hostnamectl, ["hostname", "--", name.as_str()], LIMITS)
            .await?
            .success(&self.hostnamectl)?;
        self.write_hosts(old.as_deref(), name.as_str())?;
        let (certificate_renewed, own_certificate_served) =
            match web.renew_own_certificate(&self.openssl, name).await {
                Ok(served) => (true, served),
                Err(error) => {
                    tracing::warn!(%error, "the machine's own certificate still names the old name");
                    (false, false)
                }
            };
        for unit in FOLLOWERS {
            // Not installed, or switched off: nothing to follow.
            let restarted = zendiloos_exec::run(&self.systemctl, ["try-restart", unit], LIMITS)
                .await
                .map_err(OperationError::from)
                .and_then(|output| output.success(&self.systemctl).map(|_| ()).map_err(Into::into));
            if let Err(error) = restarted {
                tracing::info!(%error, unit, "a service that reads the host name was not restarted");
            }
        }
        tracing::info!(old = ?old, new = name.as_str(), "the machine was renamed");
        Ok(Response::HostNameSet { certificate_renewed, own_certificate_served })
    }

    /// `/etc/hosts` with the new name where it had the old one, kept as it
    /// was otherwise; not written if nothing changes, and left alone if it
    /// is not a plain file (a link elsewhere is somebody else's).
    fn write_hosts(&self, old: Option<&str>, new: &str) -> Result<(), OperationError> {
        let path = &self.paths.hosts;
        let mode = match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => meta.permissions().mode() & 0o7777,
            Ok(_) => {
                tracing::warn!(path = %path.display(), "not a plain file: the host name is not written into it");
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0o644,
            Err(error) => return Err(OperationError::Read(path.clone(), error)),
        };
        let before = self.read(path)?.unwrap_or_default();
        let after = hosts_file(&before, old, new);
        if after != before {
            crate::durable::replace_mode(path, after.as_bytes(), mode)?;
        }
        Ok(())
    }

    /// A small file, if it is there.
    fn read(&self, path: &std::path::Path) -> Result<Option<String>, OperationError> {
        use std::io::Read as _;
        match fs::File::open(path) {
            Ok(file) => {
                let mut text = String::new();
                file.take(MAX_FILE)
                    .read_to_string(&mut text)
                    .map_err(|error| OperationError::Read(path.to_owned(), error))?;
                Ok(Some(text))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(OperationError::Read(path.to_owned(), error)),
        }
    }
}

/// The name `/etc/hostname` holds: its first line that is neither empty
/// nor a comment (hostname(5)).
pub(crate) fn static_name(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty() && !line.starts_with('#'))
}

/// `hosts` with `old` replaced by `new` as a name and as the first label of
/// a name (`nas.home.arpa` becomes `new.home.arpa`); the localhost names
/// are never changed. Where no line names `new` then, Debian's line for it
/// (`127.0.1.1`) says it alone, or is added after the first line of
/// `127.0.0.1` (or first): the name always resolves to the machine.
pub(crate) fn hosts_file(hosts: &str, old: Option<&str>, new: &str) -> String {
    let old = old.filter(|old| !old.is_empty() && !old.eq_ignore_ascii_case("localhost"));
    let rename = |name: &str| -> Option<String> {
        let old = old?;
        if name.eq_ignore_ascii_case(old) {
            return Some(new.to_owned());
        }
        let (first, rest) = name.split_once('.')?;
        first.eq_ignore_ascii_case(old).then(|| format!("{new}.{rest}"))
    };
    let mut lines: Vec<String> = Vec::new();
    let mut named = false;
    for line in hosts.lines() {
        let (entry, comment) = match line.find('#') {
            Some(at) => (&line[..at], &line[at..]),
            None => (line, ""),
        };
        let mut fields = entry.split_whitespace();
        let Some(address) = fields.next() else {
            lines.push(line.to_owned());
            continue;
        };
        let names: Vec<&str> = fields.collect();
        let renamed: Vec<String> =
            names.iter().map(|name| rename(name).unwrap_or_else(|| (*name).to_owned())).collect();
        named |= renamed.iter().any(|name| name.eq_ignore_ascii_case(new));
        if renamed.iter().zip(&names).all(|(after, before)| after == before) {
            lines.push(line.to_owned());
        } else {
            let comment = if comment.is_empty() { String::new() } else { format!(" {comment}") };
            lines.push(format!("{address}\t{}{comment}", renamed.join(" ")));
        }
    }
    if !named {
        let own = format!("{OWN_ADDRESS}\t{new}");
        let is = |line: &String, address: &str| line.split_whitespace().next() == Some(address);
        if let Some(at) = lines.iter().position(|line| is(line, OWN_ADDRESS)) {
            lines[at] = own;
        } else {
            let at = lines.iter().position(|line| is(line, "127.0.0.1")).map_or(0, |at| at + 1);
            lines.insert(at, own);
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEBIAN: &str = "127.0.0.1\tlocalhost\n127.0.1.1\tzendiloos\n\n\
                          # The following lines are desirable for IPv6 capable hosts\n\
                          ::1     localhost ip6-localhost ip6-loopback\nff02::1 ip6-allnodes\n";

    #[test]
    fn the_installers_line_names_the_new_name_and_nothing_else_changes() {
        let after = hosts_file(DEBIAN, Some("zendiloos"), "office");
        assert_eq!(after, DEBIAN.replace("127.0.1.1\tzendiloos", "127.0.1.1\toffice"));
    }

    #[test]
    fn a_domain_and_a_fixed_address_are_kept() {
        let hosts = "127.0.0.1 localhost\n192.168.1.10 nas.home.arpa nas # the NAS\n";
        assert_eq!(
            hosts_file(hosts, Some("NAS"), "office"),
            "127.0.0.1 localhost\n192.168.1.10\toffice.home.arpa office # the NAS\n"
        );
    }

    #[test]
    fn a_name_nowhere_gets_debians_line() {
        let hosts = "127.0.0.1 localhost\n::1 localhost\n";
        assert_eq!(
            hosts_file(hosts, Some("old"), "nas"),
            "127.0.0.1 localhost\n127.0.1.1\tnas\n::1 localhost\n"
        );
        // A line of its own, naming something else: it names this one.
        let stale = "127.0.0.1 localhost\n127.0.1.1 something\n";
        assert_eq!(hosts_file(stale, None, "nas"), "127.0.0.1 localhost\n127.0.1.1\tnas\n");
        assert_eq!(hosts_file("", None, "nas"), "127.0.1.1\tnas\n");
        // Named already: left as it is.
        assert_eq!(hosts_file("127.0.1.1 nas\n", Some("nas"), "nas"), "127.0.1.1 nas\n");
    }

    #[test]
    fn localhost_is_never_renamed() {
        let hosts = "127.0.0.1 localhost localhost.localdomain\n";
        assert_eq!(
            hosts_file(hosts, Some("localhost"), "nas"),
            "127.0.0.1 localhost localhost.localdomain\n127.0.1.1\tnas\n"
        );
    }

    #[test]
    fn the_static_name_is_the_first_line_that_says_one() {
        assert_eq!(static_name("# made by the installer\n\n  nas \n"), Some("nas"));
        assert_eq!(static_name(""), None);
    }
}
