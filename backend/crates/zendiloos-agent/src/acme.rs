//! Certificates from an ACME server, issued by acme.sh.
//!
//! acme.sh needs the network, which the agent has none of
//! (`PrivateNetwork`), and it is a shell script that should not run as
//! root: the agent starts it in a transient unit as the system account
//! `zendiloos-acme`, sandboxed, with fixed arguments made only of checked
//! protocol values, and passes on what it writes line by line. A DNS
//! provider's credentials reach it in an environment file only root reads
//! (`EnvironmentFile=`, so they show in no command line and no unit
//! property), removed when it ends. Everything acme.sh keeps lies in
//! `/var/lib/zendiloos-acme`: an account's settings and key in
//! `accounts/<n>`, a certificate's in `certificates/<n>`, and the files
//! for HTTP challenges in `webroot`, which nginx serves. acme.sh's own
//! renewal (a cron job) is never set up: zendiloosd renews.
//!
//! Once issued, the agent copies the certificate and its key to
//! `/etc/zendiloos/web/certificates/<n>`, where nginx reads them. What
//! acme.sh wrote is opened folder by folder without following a link (its
//! account owns them all and could point one elsewhere) and without
//! waiting (a pipe in place of a file is refused). The pair goes to a
//! folder beside (`<n>.zendiloos-new`), is checked there (the chain holds
//! certificates only, openssl reads each, each is issued by the one after
//! it, and the key belongs to the first), and then takes the place of the
//! pair served in one step (`RENAME_EXCHANGE`). There `nginx -t` reads it
//! as nginx would, under the lock changes of the web server take, and if
//! it refuses, the pair from before returns in one step: nginx never finds
//! half a pair, nor a pair that does not work, whatever fails.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use zendiloos_agent_proto::{AcmeChallenge, AcmeOrder, Response};
use zendiloos_exec::{Limits, Program, Surroundings};

use crate::config::Tools;
use crate::operations::OperationError;
use crate::web_server::WebServer;

/// The account acme.sh runs as.
pub const ACME_USER: &str = "zendiloos-acme";
/// How long acme.sh may take: a DNS provider's records can take minutes to
/// show.
const ISSUE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const QUICK: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };
/// Largest certificate chain or key copied.
const MAX_PEM: u64 = 256 * 1024;
/// Most certificates in a chain: the certificate and its intermediates.
const MAX_CHAIN: usize = 10;

/// Where acme.sh and what it makes are; other than the real places in tests.
#[derive(Debug, Clone)]
pub struct AcmePaths {
    /// acme.sh itself, with its DNS API scripts in `dnsapi/` beside it.
    pub script: PathBuf,
    /// What acme.sh keeps.
    pub state: PathBuf,
    /// Credentials while acme.sh runs.
    pub runtime: PathBuf,
    /// Certificates and keys as nginx reads them.
    pub certificates: PathBuf,
}

impl Default for AcmePaths {
    fn default() -> Self {
        Self {
            script: PathBuf::from("/usr/share/acme.sh/acme.sh"),
            state: PathBuf::from("/var/lib/zendiloos-acme"),
            runtime: PathBuf::from("/run/zendiloos-acme"),
            certificates: PathBuf::from("/etc/zendiloos/web/certificates"),
        }
    }
}

impl AcmePaths {
    /// The folder of certificate `n` nginx reads.
    pub fn served(&self, certificate: u32) -> PathBuf {
        self.certificates.join(certificate.to_string())
    }

    pub fn webroot(&self) -> PathBuf {
        self.state.join("webroot")
    }
}

#[derive(Debug)]
pub(crate) struct Acme {
    systemd_run: Program,
    openssl: Program,
    /// Run in a transient unit, to remove acme.sh's files of a certificate.
    rm: PathBuf,
    paths: AcmePaths,
}

/// The environment file systemd reads: `NAME="value"`, with `\`, `"`, `$`
/// and `` ` `` escaped. Values hold no control characters (the protocol
/// refuses them), so a value is one line.
pub(crate) fn environment_file(order: &AcmeOrder) -> String {
    let mut file = String::from("# Written by zendiloOS for one run of acme.sh; removed when it ends.\n");
    if let AcmeChallenge::Dns { credentials, .. } = &order.challenge {
        for (name, value) in credentials.iter() {
            let mut quoted = String::with_capacity(value.expose().len() + 2);
            for c in value.expose().chars() {
                if matches!(c, '\\' | '"' | '$' | '`') {
                    quoted.push('\\');
                }
                quoted.push(c);
            }
            file.push_str(&format!("{}=\"{quoted}\"\n", name.as_str()));
        }
    }
    file
}

/// The arguments of `systemd-run` that run acme.sh for `order`.
pub(crate) fn issue_arguments(paths: &AcmePaths, order: &AcmeOrder, environment: &Path) -> Vec<String> {
    let state = paths.state.display().to_string();
    let mut args: Vec<String> = [
        "--quiet",
        "--wait",
        "--pipe",
        "--collect",
        "--service-type=exec",
        &format!("--unit=zendiloos-acme-{}", order.certificate),
        &format!("--description=zendiloOS: certificate {} (acme.sh)", order.certificate),
        &format!("--uid={ACME_USER}"),
        &format!("--gid={ACME_USER}"),
        &format!("--working-directory={state}"),
        &format!("--setenv=HOME={state}"),
        &format!("--property=EnvironmentFile={}", environment.display()),
        // A sandbox of its own: it writes only its state, reaches only the
        // network, and gains nothing.
        "--property=ProtectSystem=strict",
        &format!("--property=ReadWritePaths={state}"),
        "--property=ProtectHome=yes",
        "--property=PrivateTmp=yes",
        "--property=PrivateDevices=yes",
        "--property=NoNewPrivileges=yes",
        "--property=CapabilityBoundingSet=",
        "--property=RestrictSUIDSGID=yes",
        "--property=RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX",
        "--property=ProtectKernelTunables=yes",
        "--property=ProtectKernelModules=yes",
        "--property=ProtectControlGroups=yes",
        "--property=LockPersonality=yes",
        // Files for HTTP challenges must be readable by nginx; accounts and
        // certificates lie in folders only the account enters.
        "--property=UMask=0022",
        &format!("--property=RuntimeMaxSec={}", ISSUE_TIMEOUT.as_secs() - 60),
        &paths.script.display().to_string(),
        "--issue",
        "--force",
        "--no-color",
        "--server",
        order.ca.server(),
        "--accountemail",
        order.email.as_str(),
        "--home",
        &state,
        "--config-home",
        &format!("{state}/accounts/{}", order.account),
        "--cert-home",
        &format!("{state}/certificates/{}", order.certificate),
        "--keylength",
        order.key.keylength(),
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for domain in order.domains.as_slice() {
        args.extend(["--domain".to_owned(), domain.as_str().to_owned()]);
    }
    match &order.challenge {
        AcmeChallenge::Dns { api, .. } => args.extend(["--dns".to_owned(), api.as_str().to_owned()]),
        AcmeChallenge::Http => args.extend(["--webroot".to_owned(), paths.webroot().display().to_string()]),
    }
    args
}

/// Opens what acme.sh wrote at `relative` below `state`: each folder on
/// the way without following a link, and the file without following one
/// nor waiting for a writer.
fn open_made(state: &Path, relative: &[&str]) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let folder = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let Some((file, folders)) = relative.split_last() else {
        return Err(io::Error::other("no file named"));
    };
    let mut at = open(state, folder, Mode::empty())?;
    for name in folders {
        at = openat(&at, *name, folder, Mode::empty())?;
    }
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
    Ok(File::from(openat(&at, *file, flags, Mode::empty())?))
}

/// Reads a file acme.sh wrote ([`open_made`]), a regular one, at most
/// [`MAX_PEM`] bytes, and only if it holds `marker`.
fn read_pem(state: &Path, relative: &[&str], marker: &str) -> Result<Vec<u8>, OperationError> {
    let path = relative.iter().fold(state.to_owned(), |path, name| path.join(name));
    let read = || -> io::Result<Vec<u8>> {
        let file = open_made(state, relative)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        let mut content = Vec::new();
        file.take(MAX_PEM + 1).read_to_end(&mut content)?;
        if content.len() as u64 > MAX_PEM {
            return Err(io::Error::other("too large"));
        }
        Ok(content)
    };
    let content = read().map_err(|e| OperationError::Read(path.clone(), e))?;
    if !String::from_utf8_lossy(&content).contains(marker) {
        return Err(OperationError::Failed(format!("{} is not what acme.sh writes", path.display())));
    }
    Ok(content)
}

/// Puts `folder`'s entries, and what happened to them, on disk.
fn sync_folder(folder: &Path) -> Result<(), OperationError> {
    File::open(folder).and_then(|f| f.sync_all()).map_err(|e| OperationError::Write(folder.to_owned(), e))
}

/// Writes `content` to `path` with `mode`, replacing it whole.
fn write_file(path: &Path, content: &[u8], mode: u32) -> Result<(), OperationError> {
    let temporary = path.with_extension("zendiloos-new");
    let written = (|| {
        let _ = fs::remove_file(&temporary);
        let mut file = OpenOptions::new().write(true).create_new(true).mode(mode).open(&temporary)?;
        file.write_all(content)?;
        file.sync_all()
    })();
    written.map_err(|e| OperationError::Write(temporary.clone(), e))?;
    fs::rename(&temporary, path).map_err(|e| OperationError::Write(path.to_owned(), e))
}

/// The certificates of `chain`, each PEM block on its own, in order; `None`
/// if it holds anything else, or none. nginx reads every block of the file
/// it is given, and does not start on one it cannot read.
pub(crate) fn chain_blocks(chain: &str) -> Option<Vec<&str>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut blocks = Vec::new();
    let mut rest = chain.trim_start();
    while !rest.is_empty() {
        let after = rest.strip_prefix(BEGIN)?;
        let (body, _) = after.split_once(END)?;
        if !body.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=\r\n".contains(&b)) {
            return None;
        }
        let length = BEGIN.len() + body.len() + END.len();
        blocks.push(&rest[..length]);
        rest = rest[length..].trim_start();
    }
    (!blocks.is_empty()).then_some(blocks)
}

const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Unix seconds of OpenSSL's `notAfter=Feb  6 12:00:00 2027 GMT`.
pub(crate) fn not_after(text: &str) -> Option<i64> {
    let date = text.trim().strip_prefix("notAfter=")?;
    let parts: Vec<&str> = date.split_whitespace().collect();
    let [month, day, time, year, "GMT"] = parts.as_slice() else { return None };
    let month = i64::try_from(MONTHS.iter().position(|m| m == month)? + 1).ok()?;
    let day: i64 = day.parse().ok()?;
    let year: i64 = year.parse().ok()?;
    let clock: Vec<i64> = time.split(':').map(str::parse).collect::<Result<_, _>>().ok()?;
    let [hours, minutes, seconds] = clock.as_slice() else { return None };
    if !(1..=31).contains(&day) || *hours > 23 || *minutes > 59 || *seconds > 60 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hours * 3600 + minutes * 60 + seconds)
}

/// The last line acme.sh wrote that says what went wrong.
fn complaint(written: &str) -> String {
    let lines: Vec<&str> = written.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let error =
        lines.iter().rev().find(|l| l.contains("error") || l.contains("Error") || l.contains("failed"));
    let line = error.or(lines.last()).copied().unwrap_or("acme.sh failed");
    // acme.sh starts each line with the time: `[Sat Nov  7 12:00:00 UTC 2026] …`.
    let line = line.split_once("] ").filter(|_| line.starts_with('[')).map_or(line, |(_, rest)| rest);
    format!("acme.sh: {line}")
}

impl Acme {
    pub(crate) fn new(tools: &Tools, paths: AcmePaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            systemd_run: Program::new(&tools.systemd_run)?,
            openssl: Program::new(&tools.openssl)?,
            rm: tools.rm.clone(),
            paths,
        })
    }

    pub(crate) fn installed(&self) -> bool {
        self.paths.script.is_file()
    }

    pub(crate) fn state(&self) -> Response {
        Response::AcmeState { installed: self.installed() }
    }

    /// Issues the certificate `order` asks for, and puts it where `nginx`
    /// reads it once both openssl and nginx take it.
    pub(crate) async fn issue(
        &self,
        order: &AcmeOrder,
        nginx: &WebServer,
    ) -> Result<Response, OperationError> {
        if !self.installed() {
            return Err(OperationError::Setup("acme.sh is not installed".into()));
        }
        if let AcmeChallenge::Dns { api, .. } = &order.challenge {
            let script = self.paths.script.with_file_name("dnsapi").join(format!("{}.sh", api.as_str()));
            if !script.is_file() {
                return Err(OperationError::Refused(format!("acme.sh has no DNS API {}", api.as_str())));
            }
        }
        crate::progress::step("issuing", Some(10));
        fs::create_dir_all(&self.paths.runtime)
            .map_err(|e| OperationError::Write(self.paths.runtime.clone(), e))?;
        let _ = fs::set_permissions(&self.paths.runtime, fs::Permissions::from_mode(0o700));
        let environment = self.paths.runtime.join(format!("{}.env", order.certificate));
        write_file(&environment, environment_file(order).as_bytes(), 0o600)?;
        let args = issue_arguments(&self.paths, order, &environment);
        let limits = Limits { timeout: ISSUE_TIMEOUT, max_stdout: 0 };
        let mut written = String::new();
        let mut line = |text: &str| {
            crate::progress::line(text);
            written.push_str(text);
            written.push('\n');
            let excess = written.len().saturating_sub(16 * 1024);
            if excess > 0 {
                let cut = written.ceil_char_boundary(excess);
                written.drain(..cut);
            }
        };
        let ran =
            zendiloos_exec::run_lines(&self.systemd_run, &args, &Surroundings::default(), limits, &mut line)
                .await;
        let _ = fs::remove_file(&environment);
        let output = ran?;
        if !output.status.success() {
            return Err(OperationError::Failed(complaint(&written)));
        }
        crate::progress::step("installing", Some(90));
        let staged = self.stage(order)?;
        let checked = self.check(&staged).await;
        let not_after = match checked {
            Ok(not_after) => not_after,
            Err(error) => {
                // The pair served stays as it was.
                let _ = fs::remove_dir_all(&staged);
                return Err(error);
            }
        };
        self.put_in_place(order.certificate, &staged, nginx).await?;
        Ok(Response::CertificateIssued { not_after })
    }

    /// Where a new pair of certificate `n` waits to be checked.
    fn staged(&self, certificate: u32) -> PathBuf {
        self.paths.certificates.join(format!("{certificate}.zendiloos-new"))
    }

    /// Copies what acme.sh made into [`Acme::staged`], on disk; that
    /// folder.
    fn stage(&self, order: &AcmeOrder) -> Result<PathBuf, OperationError> {
        let main = order.domains.main().as_str();
        let folder = if order.key.is_ecc() { format!("{main}_ecc") } else { main.to_owned() };
        let certificate = order.certificate.to_string();
        let made = ["certificates", certificate.as_str(), folder.as_str()];
        let key_name = format!("{main}.key");
        let chain =
            read_pem(&self.paths.state, &[&made[..], &["fullchain.cer"]].concat(), "BEGIN CERTIFICATE")?;
        let key = read_pem(&self.paths.state, &[&made[..], &[key_name.as_str()]].concat(), "PRIVATE KEY")?;
        let certificates = &self.paths.certificates;
        fs::create_dir_all(certificates).map_err(|e| OperationError::Write(certificates.clone(), e))?;
        let staged = self.staged(order.certificate);
        match fs::remove_dir_all(&staged) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(OperationError::Write(staged, error));
            }
            _ => {}
        }
        fs::create_dir(&staged).map_err(|e| OperationError::Write(staged.clone(), e))?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o700))
            .map_err(|e| OperationError::Write(staged.clone(), e))?;
        write_file(&staged.join("key.pem"), &key, 0o600)?;
        write_file(&staged.join("fullchain.pem"), &chain, 0o644)?;
        sync_folder(&staged)?;
        Ok(staged)
    }

    /// The pair in `folder` works: the chain holds certificates only, each
    /// of which openssl reads, each issued by the one after it, and the key
    /// is the first's; until when that one holds. Each certificate is
    /// checked in a file of its own, beside the pair, removed afterwards.
    async fn check(&self, folder: &Path) -> Result<i64, OperationError> {
        let chain = folder.join("fullchain.pem");
        let text = fs::read_to_string(&chain).map_err(|e| OperationError::Read(chain.clone(), e))?;
        let blocks = chain_blocks(&text)
            .filter(|blocks| blocks.len() <= MAX_CHAIN)
            .ok_or_else(|| OperationError::Failed("the chain holds something besides certificates".into()))?;
        let mut files = Vec::new();
        let mut written = Ok(());
        for (n, block) in blocks.iter().enumerate() {
            let file = folder.join(format!("chain.{n}.pem"));
            written = write_file(&file, block.as_bytes(), 0o600);
            if written.is_err() {
                break;
            }
            files.push(file);
        }
        let checked = match written {
            Ok(()) => self.check_chain(&files, &folder.join("key.pem")).await,
            Err(error) => Err(error),
        };
        for file in &files {
            let _ = fs::remove_file(file);
        }
        checked
    }

    /// See [`Acme::check`]: `certificates` are the chain's, the first one
    /// the certificate itself.
    async fn check_chain(&self, certificates: &[PathBuf], key: &Path) -> Result<i64, OperationError> {
        let shown: Vec<String> = certificates.iter().map(|file| file.display().to_string()).collect();
        let Some(first) = shown.first() else {
            return Err(OperationError::Failed("the chain holds no certificate".into()));
        };
        let end = self.openssl(&["x509", "-noout", "-enddate", "-in", first]).await?;
        let until = not_after(&end)
            .ok_or_else(|| OperationError::Failed(format!("openssl: cannot read {}", end.trim())))?;
        for (n, file) in shown.iter().enumerate().skip(1) {
            if self.openssl(&["x509", "-noout", "-in", file]).await.is_err() {
                return Err(OperationError::Failed(format!(
                    "certificate {} of the chain cannot be read",
                    n + 1
                )));
            }
        }
        // Each is checked against the next alone (`-partial_chain`), so none
        // is skipped and no other certificate of the system's counts.
        for (n, pair) in shown.windows(2).enumerate() {
            let args = ["verify", "-no_check_time", "-partial_chain", "-trusted", &pair[1], &pair[0]];
            if self.openssl(&args).await.is_err() {
                return Err(OperationError::Failed(format!(
                    "certificate {} of the chain is not issued by the one after it",
                    n + 1
                )));
            }
        }
        let certificate = self.openssl(&["x509", "-noout", "-pubkey", "-in", first]).await?;
        let keys = self.openssl(&["pkey", "-pubout", "-in", &key.display().to_string()]).await?;
        if certificate.trim().is_empty() || certificate != keys {
            return Err(OperationError::Failed("the certificate and its key do not belong together".into()));
        }
        Ok(until)
    }

    async fn openssl(&self, args: &[&str]) -> Result<String, OperationError> {
        let output = zendiloos_exec::run(&self.openssl, args, QUICK).await?.success(&self.openssl)?;
        String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)
    }

    /// Puts the pair checked in `staged` where nginx reads it, in one step,
    /// and has `nginx -t` read it there: then the one before goes; refused,
    /// the one before returns, in one step too, so nginx is never left with
    /// a pair it would not start with.
    async fn put_in_place(
        &self,
        certificate: u32,
        staged: &Path,
        nginx: &WebServer,
    ) -> Result<(), OperationError> {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        use rustix::io::Errno;
        let served = self.paths.served(certificate);
        let write = |error: io::Error| OperationError::Write(served.clone(), error);
        // Held from the swap until it stands or is undone: no change of the
        // web server, and no reload, meets the new pair before nginx -t did.
        let held = nginx.hold().await;
        let exchanged = match renameat_with(CWD, staged, CWD, &served, RenameFlags::EXCHANGE) {
            // The pair before is in `staged` now.
            Ok(()) => true,
            Err(Errno::NOENT) => {
                fs::rename(staged, &served).map_err(write)?;
                false
            }
            Err(error) => return Err(write(error.into())),
        };
        sync_folder(&self.paths.certificates)?;
        if let Err(refused) = held.check().await {
            let back = if exchanged {
                renameat_with(CWD, staged, CWD, &served, RenameFlags::EXCHANGE).map_err(io::Error::from)
            } else {
                fs::rename(&served, staged)
            };
            back.map_err(write)?;
            let _ = fs::remove_dir_all(staged);
            sync_folder(&self.paths.certificates)?;
            return Err(OperationError::Failed(format!(
                "nginx does not take the new certificate: {refused}"
            )));
        }
        if exchanged {
            fs::remove_dir_all(staged).map_err(write)?;
        }
        sync_folder(&self.paths.certificates)
    }

    /// Removes what acme.sh and nginx kept of a certificate.
    pub(crate) async fn remove(&self, certificate: u32) -> Result<Response, OperationError> {
        self.remove_kept(&self.paths.state.join("certificates").join(certificate.to_string())).await?;
        for served in [self.paths.served(certificate), self.staged(certificate)] {
            match fs::remove_dir_all(&served) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(OperationError::Write(served, error)),
            }
        }
        Ok(Response::Done)
    }

    /// Removes what acme.sh kept of an account: its key and settings.
    pub(crate) async fn remove_account(&self, account: u32) -> Result<Response, OperationError> {
        self.remove_kept(&self.paths.state.join("accounts").join(account.to_string())).await?;
        Ok(Response::Done)
    }

    /// Removes a folder of acme.sh's. It belongs to its account, and the
    /// agent may read anything but write only into what is root's: that
    /// account removes the folder, in a unit of its own.
    async fn remove_kept(&self, kept: &Path) -> Result<(), OperationError> {
        if fs::symlink_metadata(kept).is_ok() {
            let (state, kept) = (self.paths.state.display().to_string(), kept.display().to_string());
            let (rm, uid, gid) =
                (self.rm.display().to_string(), format!("--uid={ACME_USER}"), format!("--gid={ACME_USER}"));
            let writable = format!("--property=ReadWritePaths={state}");
            let args = [
                "--quiet",
                "--wait",
                "--pipe",
                "--collect",
                "--service-type=exec",
                uid.as_str(),
                gid.as_str(),
                "--property=ProtectSystem=strict",
                writable.as_str(),
                "--property=NoNewPrivileges=yes",
                "--property=PrivateNetwork=yes",
                rm.as_str(),
                "--recursive",
                "--force",
                "--one-file-system",
                "--",
                kept.as_str(),
            ];
            zendiloos_exec::run(&self.systemd_run, args, QUICK).await?.success(&self.systemd_run)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use zendiloos_agent_proto::{
        AcmeCa, AcmeDomain, AcmeDomains, AcmeEmail, AcmeKey, DnsApi, DnsCredentials, DnsValue,
        DnsVariableName,
    };

    use super::*;

    fn order(challenge: AcmeChallenge, key: AcmeKey) -> AcmeOrder {
        AcmeOrder {
            certificate: 3,
            account: 1,
            ca: AcmeCa::LetsEncryptStaging,
            email: AcmeEmail::parse("admin@example.org").unwrap(),
            domains: AcmeDomains::new(vec![
                AcmeDomain::parse("nas.example.org").unwrap(),
                AcmeDomain::parse("files.example.org").unwrap(),
            ])
            .unwrap(),
            key,
            challenge,
        }
    }

    fn ionos(secret: &str) -> AcmeChallenge {
        let values = BTreeMap::from([
            (DnsVariableName::parse("IONOS_PREFIX").unwrap(), DnsValue::new("prefix").unwrap()),
            (DnsVariableName::parse("IONOS_SECRET").unwrap(), DnsValue::new(secret).unwrap()),
        ]);
        AcmeChallenge::Dns {
            api: DnsApi::parse("dns_ionos").unwrap(),
            credentials: DnsCredentials::new(values).unwrap(),
        }
    }

    #[test]
    fn acme_sh_gets_fixed_arguments_and_the_secrets_only_in_its_environment() {
        let paths = AcmePaths::default();
        let dns = order(ionos("s3cr3t"), AcmeKey::Ec256);
        let args = issue_arguments(&paths, &dns, Path::new("/run/zendiloos-acme/3.env"));
        let joined = args.join(" ");
        assert!(!joined.contains("s3cr3t") && !joined.contains("prefix"), "{joined}");
        for expected in [
            "--uid=zendiloos-acme",
            "--property=EnvironmentFile=/run/zendiloos-acme/3.env",
            "--property=ReadWritePaths=/var/lib/zendiloos-acme",
            "/usr/share/acme.sh/acme.sh --issue --force --no-color --server letsencrypt_test",
            "--config-home /var/lib/zendiloos-acme/accounts/1",
            "--cert-home /var/lib/zendiloos-acme/certificates/3",
            "--keylength ec-256 --domain nas.example.org --domain files.example.org --dns dns_ionos",
        ] {
            assert!(joined.contains(expected), "{expected} in {joined}");
        }
        let http = issue_arguments(&paths, &order(AcmeChallenge::Http, AcmeKey::Rsa2048), Path::new("/x"));
        assert!(http.join(" ").ends_with("--keylength 2048 --domain nas.example.org --domain files.example.org --webroot /var/lib/zendiloos-acme/webroot"));

        let file = environment_file(&order(ionos(r#"a"b\c$d`e"#), AcmeKey::Ec256));
        assert!(file.ends_with("IONOS_PREFIX=\"prefix\"\nIONOS_SECRET=\"a\\\"b\\\\c\\$d\\`e\"\n"), "{file}");
        assert_eq!(environment_file(&order(AcmeChallenge::Http, AcmeKey::Ec256)).lines().count(), 1);
    }

    #[test]
    fn openssl_dates_are_read() {
        assert_eq!(not_after("notAfter=Jan  1 00:00:00 1970 GMT\n"), Some(0));
        assert_eq!(not_after("notAfter=Feb  6 12:30:15 2027 GMT"), Some(1_801_917_015));
        assert_eq!(not_after("notAfter=Mar  1 00:00:00 2028 GMT"), Some(1_835_481_600));
        for bad in [
            "",
            "notAfter=Feb 6 2027",
            "notAfter=Foo  6 12:00:00 2027 GMT",
            "notAfter=Feb 32 12:00:00 2027 GMT",
        ] {
            assert_eq!(not_after(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_complaint_is_the_line_that_says_what_failed() {
        let written = "[Sat Nov  7 12:00:00 UTC 2026] Getting domain auth token\n\
                       [Sat Nov  7 12:00:05 UTC 2026] Error add txt for domain:_acme-challenge.nas.example.org\n\
                       [Sat Nov  7 12:00:05 UTC 2026] Please add '--debug' or '--log' to see more information.\n";
        assert_eq!(complaint(written), "acme.sh: Error add txt for domain:_acme-challenge.nas.example.org");
        assert_eq!(complaint(""), "acme.sh: acme.sh failed");
    }

    /// acme.sh and systemd-run as scripts in `dir`: systemd-run runs the
    /// last arguments' script after `--property=…` options; acme.sh writes
    /// a certificate for its `--cert-home`, or fails while `fail` is there.
    fn acme(dir: &Path) -> Acme {
        let executable = |path: &Path, text: &str| {
            fs::write(path, text).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        };
        let paths = AcmePaths {
            script: dir.join("share/acme.sh"),
            state: dir.join("state"),
            runtime: dir.join("run"),
            certificates: dir.join("web/certificates"),
        };
        fs::create_dir_all(dir.join("share/dnsapi")).unwrap();
        fs::write(dir.join("share/dnsapi/dns_ionos.sh"), "").unwrap();
        let fail = dir.join("fail");
        executable(
            &paths.script,
            &format!(
                "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = --cert-home ] && home=$2; [ \"$1\" = --domain ] && [ -z \"$main\" ] && main=$2; shift; done\n\
                 echo \"[Sat Nov  7 12:00:00 UTC 2026] Creating domain key\"\n\
                 if [ -f {fail} ]; then echo \"[Sat Nov  7 12:00:01 UTC 2026] Error add txt for domain\"; exit 1; fi\n\
                 mkdir -p \"$home/${{main}}_ecc\"\nprintf -- '-----BEGIN CERTIFICATE-----\\nMIIB\\n-----END CERTIFICATE-----\\n' > \"$home/${{main}}_ecc/fullchain.cer\"\n\
                 printf -- '-----BEGIN PRIVATE KEY-----\\n' > \"$home/${{main}}_ecc/$main.key\"\necho Cert success.\n",
                fail = fail.display()
            ),
        );
        executable(
            &dir.join("systemd-run"),
            &format!(
                "#!/bin/sh\nfor a; do case \"$a\" in --property=EnvironmentFile=*) cp \"${{a##*=}}\" {seen};; esac; done\n\
                 while case \"$1\" in --*) true;; *) false;; esac; do shift; done\nexec \"$@\"\n",
                seen = dir.join("seen.env").display(),
            ),
        );
        executable(&dir.join("openssl"), "#!/bin/sh\necho 'notAfter=Feb  6 12:30:15 2027 GMT'\n");
        let tools = Tools {
            systemd_run: dir.join("systemd-run"),
            openssl: dir.join("openssl"),
            rm: "/usr/bin/rm".into(),
            ..Tools::default()
        };
        Acme::new(&tools, paths).unwrap()
    }

    /// nginx as `nginx -t` sees it, through [`acme`]'s systemd-run: it
    /// refuses while `nginx-refuses` is there.
    fn nginx(dir: &Path) -> WebServer {
        let nginx = dir.join("nginx");
        fs::write(
            &nginx,
            format!(
                "#!/bin/sh\nif [ -f {} ]; then echo 'nginx: [emerg] cannot load certificate'; exit 1; fi\n",
                dir.join("nginx-refuses").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&nginx, fs::Permissions::from_mode(0o755)).unwrap();
        let tools = Tools {
            systemd_run: dir.join("systemd-run"),
            systemctl: "/bin/false".into(),
            nginx,
            ..Tools::default()
        };
        WebServer::new(&tools, crate::web_server::WebServerPaths::default()).unwrap()
    }

    #[tokio::test]
    async fn a_certificate_is_issued_copied_for_nginx_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (acme, nginx) = (acme(dir.path()), nginx(dir.path()));
        assert_eq!(acme.state(), Response::AcmeState { installed: true });
        let dns = order(ionos("s3cr3t"), AcmeKey::Ec256);
        assert_eq!(
            acme.issue(&dns, &nginx).await.unwrap(),
            Response::CertificateIssued { not_after: 1_801_917_015 }
        );
        let served = dir.path().join("web/certificates/3");
        assert!(fs::read_to_string(served.join("fullchain.pem")).unwrap().contains("BEGIN CERTIFICATE"));
        let mode = fs::metadata(served.join("key.pem")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // The credentials were there while it ran, and are gone now.
        assert!(fs::read_to_string(dir.path().join("seen.env")).unwrap().contains("IONOS_SECRET=\"s3cr3t\""));
        assert!(!dir.path().join("run/3.env").exists());

        fs::write(dir.path().join("fail"), "").unwrap();
        let failed = acme.issue(&dns, &nginx).await.unwrap_err();
        assert_eq!(failed.to_string(), "acme.sh: Error add txt for domain");
        assert!(!dir.path().join("run/3.env").exists());

        let unknown = AcmeChallenge::Dns {
            api: DnsApi::parse("dns_nope").unwrap(),
            credentials: DnsCredentials::default(),
        };
        assert!(
            acme.issue(&order(unknown, AcmeKey::Ec256), &nginx)
                .await
                .unwrap_err()
                .to_string()
                .contains("no DNS API")
        );

        assert_eq!(acme.remove(3).await.unwrap(), Response::Done);
        assert!(!served.exists() && !dir.path().join("state/certificates/3").exists());
        // An account's key and settings go with it.
        fs::create_dir_all(dir.path().join("state/accounts/1")).unwrap();
        fs::write(dir.path().join("state/accounts/1/account.key"), "key").unwrap();
        assert_eq!(acme.remove_account(1).await.unwrap(), Response::Done);
        assert!(!dir.path().join("state/accounts/1").exists());
        assert_eq!(acme.remove_account(1).await.unwrap(), Response::Done, "nothing left is fine");
        assert_eq!(acme.remove(3).await.unwrap(), Response::Done, "nothing left is fine");
        fs::remove_file(dir.path().join("share/acme.sh")).unwrap();
        assert_eq!(acme.state(), Response::AcmeState { installed: false });
        assert!(acme.issue(&dns, &nginx).await.unwrap_err().to_string().contains("not installed"));
    }

    #[tokio::test]
    async fn files_acme_sh_points_elsewhere_are_not_copied() {
        let dir = tempfile::tempdir().unwrap();
        let acme = acme(dir.path());
        let made = dir.path().join("state/certificates/3/nas.example.org_ecc");
        fs::create_dir_all(&made).unwrap();
        fs::write(dir.path().join("secret"), "-----BEGIN PRIVATE KEY-----").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), made.join("fullchain.cer")).unwrap();
        fs::write(made.join("nas.example.org.key"), "-----BEGIN PRIVATE KEY-----").unwrap();
        assert!(acme.stage(&order(AcmeChallenge::Http, AcmeKey::Ec256)).is_err());
        fs::remove_file(made.join("fullchain.cer")).unwrap();
        fs::write(made.join("fullchain.cer"), "not a certificate").unwrap();
        assert!(acme.stage(&order(AcmeChallenge::Http, AcmeKey::Ec256)).is_err());
    }

    /// acme.sh's account owns its folders: one made a link to elsewhere is
    /// not followed, and a pipe in place of the chain is refused at once,
    /// not waited on.
    #[tokio::test]
    async fn folders_acme_sh_points_elsewhere_and_pipes_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let acme = acme(dir.path());
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("fullchain.cer"), "-----BEGIN CERTIFICATE-----").unwrap();
        fs::write(elsewhere.join("nas.example.org.key"), "-----BEGIN PRIVATE KEY----- not acme's").unwrap();
        let mine = dir.path().join("state/certificates/3");
        fs::create_dir_all(&mine).unwrap();
        std::os::unix::fs::symlink(&elsewhere, mine.join("nas.example.org_ecc")).unwrap();
        assert!(acme.stage(&order(AcmeChallenge::Http, AcmeKey::Ec256)).is_err());
        fs::remove_file(mine.join("nas.example.org_ecc")).unwrap();

        let made = mine.join("nas.example.org_ecc");
        fs::create_dir_all(&made).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            made.join("fullchain.cer"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o644),
            0,
        )
        .unwrap();
        fs::write(made.join("nas.example.org.key"), "-----BEGIN PRIVATE KEY-----").unwrap();
        let refused = tokio::time::timeout(Duration::from_secs(5), async {
            acme.stage(&order(AcmeChallenge::Http, AcmeKey::Ec256))
        })
        .await
        .expect("not waiting for a writer");
        assert!(
            matches!(&refused, Err(OperationError::Read(_, e)) if e.to_string() == "not a regular file"),
            "{refused:?}"
        );
    }

    /// A pair that does not work (openssl cannot read the chain, or the
    /// key is another's) never takes the place of the one served.
    #[tokio::test]
    async fn a_pair_that_does_not_work_leaves_the_one_served() {
        let dir = tempfile::tempdir().unwrap();
        let (acme, nginx) = (acme(dir.path()), nginx(dir.path()));
        let dns = order(ionos("s3cr3t"), AcmeKey::Ec256);
        acme.issue(&dns, &nginx).await.unwrap();
        let served = dir.path().join("web/certificates/3");
        fs::write(served.join("fullchain.pem"), "-----BEGIN CERTIFICATE----- working").unwrap();
        fs::write(served.join("key.pem"), "-----BEGIN PRIVATE KEY----- working").unwrap();
        let openssl = |script: &str| {
            fs::write(dir.path().join("openssl"), script).unwrap();
            fs::set_permissions(dir.path().join("openssl"), fs::Permissions::from_mode(0o755)).unwrap();
        };

        openssl("#!/bin/sh\necho 'unable to load certificate' >&2\nexit 1\n");
        assert!(acme.issue(&dns, &nginx).await.is_err());
        // The key is another's: its public key differs from the certificate's.
        openssl(
            "#!/bin/sh\ncase \"$*\" in *-enddate*) echo 'notAfter=Feb  6 12:30:15 2027 GMT';; \
             *-pubkey*) echo certificate;; *) echo other;; esac\n",
        );
        let mismatched = acme.issue(&dns, &nginx).await.unwrap_err();
        assert_eq!(mismatched.to_string(), "the certificate and its key do not belong together");
        assert!(fs::read_to_string(served.join("fullchain.pem")).unwrap().contains("working"));
        assert!(fs::read_to_string(served.join("key.pem")).unwrap().contains("working"));
        assert!(!dir.path().join("web/certificates/3.zendiloos-new").exists());

        // A pair that works replaces it whole.
        openssl(
            "#!/bin/sh\ncase \"$*\" in *-enddate*) echo 'notAfter=Feb  6 12:30:15 2027 GMT';; *) echo same;; esac\n",
        );
        acme.issue(&dns, &nginx).await.unwrap();
        assert!(!fs::read_to_string(served.join("fullchain.pem")).unwrap().contains("working"));
        assert!(!fs::read_to_string(served.join("key.pem")).unwrap().contains("working"));
        assert!(!dir.path().join("web/certificates/3.zendiloos-new").exists());
    }

    /// nginx reads the new pair where it is served once it took the place
    /// of the one before; should `nginx -t` refuse it there, the one before
    /// returns, and nginx starts next time as it did.
    #[tokio::test]
    async fn a_pair_nginx_refuses_is_swapped_back() {
        let dir = tempfile::tempdir().unwrap();
        let (acme, nginx) = (acme(dir.path()), nginx(dir.path()));
        let dns = order(ionos("s3cr3t"), AcmeKey::Ec256);
        acme.issue(&dns, &nginx).await.unwrap();
        let served = dir.path().join("web/certificates/3");
        fs::write(served.join("fullchain.pem"), "-----BEGIN CERTIFICATE----- working").unwrap();
        fs::write(served.join("key.pem"), "-----BEGIN PRIVATE KEY----- working").unwrap();

        fs::write(dir.path().join("nginx-refuses"), "").unwrap();
        let refused = acme.issue(&dns, &nginx).await.unwrap_err();
        assert_eq!(
            refused.to_string(),
            "nginx does not take the new certificate: nginx: [emerg] cannot load certificate"
        );
        assert!(fs::read_to_string(served.join("fullchain.pem")).unwrap().contains("working"));
        assert!(fs::read_to_string(served.join("key.pem")).unwrap().contains("working"));
        assert!(!dir.path().join("web/certificates/3.zendiloos-new").exists());

        // Never served before: nothing is left in its place.
        let other = AcmeOrder { certificate: 4, ..dns.clone() };
        assert!(acme.issue(&other, &nginx).await.is_err());
        assert!(!dir.path().join("web/certificates/4").exists());
        assert!(!dir.path().join("web/certificates/4.zendiloos-new").exists());
    }

    #[test]
    fn a_chain_is_certificates_and_nothing_else() {
        let block = |body: &str| format!("-----BEGIN CERTIFICATE-----\n{body}\n-----END CERTIFICATE-----\n");
        let chain = format!("{}\n{}", block("MIIB/w=="), block("MIIC\r\nAA+b"));
        let blocks = chain_blocks(&chain).unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(blocks[1].starts_with("-----BEGIN") && blocks[1].ends_with("CERTIFICATE-----"));
        for odd in [
            String::new(),
            "\n\n".to_owned(),
            format!("{}-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n", block("MIIB")),
            format!("{}garbage\n", block("MIIB")),
            format!("subject=CN = nas\n{}", block("MIIB")),
            block("not*base64*at*all"),
            "-----BEGIN CERTIFICATE-----\nMIIB\n".to_owned(),
            block("MIIB\n-----BEGIN CERTIFICATE-----\nMIIC"),
        ] {
            assert!(chain_blocks(&odd).is_none(), "{odd:?}");
        }
    }

    /// With the real openssl: every certificate of the chain is read, each
    /// must be issued by the one after it, and the key is the first's. nginx
    /// reads every block of the chain and does not start on one it cannot
    /// read; a chain of certificates that do not belong together is no use
    /// to a browser.
    #[tokio::test]
    async fn every_certificate_of_the_chain_is_read_and_follows_from_the_next() {
        let openssl = PathBuf::from("/usr/bin/openssl");
        if !openssl.exists() {
            eprintln!("no openssl here: skipped");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("openssl.cnf");
        fs::write(
            &config,
            "[req]\ndistinguished_name = dn\n[dn]\n\
             [ca]\nbasicConstraints = critical,CA:TRUE\nkeyUsage = critical,keyCertSign\nsubjectKeyIdentifier = hash\n\
             [leaf]\nbasicConstraints = critical,CA:FALSE\nauthorityKeyIdentifier = keyid\n",
        )
        .unwrap();
        let run = |args: &[&str]| {
            let made =
                std::process::Command::new(&openssl).args(args).current_dir(dir.path()).output().unwrap();
            assert!(made.status.success(), "{args:?}: {}", String::from_utf8_lossy(&made.stderr));
        };
        let ec = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-config", "openssl.cnf"];
        for (name, subject) in [("ca", "/CN=Test CA"), ("other", "/CN=Other CA")] {
            let (key, certificate) = (format!("{name}.key"), format!("{name}.pem"));
            run(&[
                &["req", "-x509", "-extensions", "ca", "-days", "2", "-subj", subject][..],
                &ec[..],
                &["-keyout", &key, "-out", &certificate],
            ]
            .concat());
        }
        run(&[
            &["req", "-new", "-subj", "/CN=nas.example.org"][..],
            &ec[..],
            &["-keyout", "leaf.key", "-out", "leaf.csr"],
        ]
        .concat());
        let sign = "x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -set_serial 2 -days 2 \
                    -extfile openssl.cnf -extensions leaf -out leaf.pem";
        run(&sign.split_whitespace().collect::<Vec<_>>());
        let read = |name: &str| fs::read_to_string(dir.path().join(name)).unwrap();
        let tools = Tools {
            openssl: openssl.clone(),
            systemd_run: "/bin/false".into(),
            rm: "/usr/bin/rm".into(),
            ..Tools::default()
        };
        let paths = AcmePaths {
            script: dir.path().join("acme.sh"),
            state: dir.path().join("state"),
            runtime: dir.path().join("run"),
            certificates: dir.path().join("certificates"),
        };
        let acme = Acme::new(&tools, paths).unwrap();
        let pair = dir.path().join("pair");
        fs::create_dir_all(&pair).unwrap();
        fs::write(pair.join("key.pem"), read("leaf.key")).unwrap();
        let check = |chain: String| {
            fs::write(pair.join("fullchain.pem"), chain).unwrap();
            acme.check(&pair)
        };

        assert!(check(read("leaf.pem") + &read("ca.pem")).await.is_ok());
        assert!(check(read("leaf.pem")).await.is_ok(), "the certificate alone");
        let broken = "-----BEGIN CERTIFICATE-----\nTUlJQgo=\n-----END CERTIFICATE-----\n";
        assert_eq!(
            check(read("leaf.pem") + broken).await.unwrap_err().to_string(),
            "certificate 2 of the chain cannot be read"
        );
        assert_eq!(
            check(read("leaf.pem") + &read("other.pem")).await.unwrap_err().to_string(),
            "certificate 1 of the chain is not issued by the one after it"
        );
        assert!(check(read("leaf.pem") + &read("ca.pem") + &read("other.pem")).await.is_err());
        assert_eq!(
            check(read("leaf.pem") + &read("ca.key")).await.unwrap_err().to_string(),
            "the chain holds something besides certificates"
        );
        assert!(check(read("ca.pem") + &read("leaf.pem")).await.is_err(), "the key is the first's");
        let left: Vec<_> = fs::read_dir(&pair).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 2, "only the pair is left: {left:?}");
    }
}
