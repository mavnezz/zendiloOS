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
//! `/etc/zendiloos/web/certificates/<n>`, where nginx reads them: opened
//! without following links, so the account cannot point the agent at
//! another file.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use zendiloos_agent_proto::{AcmeChallenge, AcmeOrder, Response};
use zendiloos_exec::{Limits, Program, Surroundings};

use crate::config::Tools;
use crate::operations::OperationError;

/// The account acme.sh runs as.
pub const ACME_USER: &str = "zendiloos-acme";
/// How long acme.sh may take: a DNS provider's records can take minutes to
/// show.
const ISSUE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const QUICK: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };
/// Largest certificate chain or key copied.
const MAX_PEM: u64 = 256 * 1024;

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

/// Reads a file acme.sh wrote, not following a link, at most [`MAX_PEM`]
/// bytes, and only if it holds `marker`.
fn read_pem(path: &Path, marker: &str) -> Result<Vec<u8>, OperationError> {
    let read = || -> io::Result<Vec<u8>> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)?;
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
    let content = read().map_err(|e| OperationError::Read(path.to_owned(), e))?;
    if !String::from_utf8_lossy(&content).contains(marker) {
        return Err(OperationError::Failed(format!("{} is not what acme.sh writes", path.display())));
    }
    Ok(content)
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
            paths,
        })
    }

    pub(crate) fn installed(&self) -> bool {
        self.paths.script.is_file()
    }

    pub(crate) fn state(&self) -> Response {
        Response::AcmeState { installed: self.installed() }
    }

    pub(crate) async fn issue(&self, order: &AcmeOrder) -> Result<Response, OperationError> {
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
        let not_after = self.install(order)?;
        let not_after = self.expiry(&not_after).await?;
        Ok(Response::CertificateIssued { not_after })
    }

    /// Copies what acme.sh made to where nginx reads it; the chain's path.
    fn install(&self, order: &AcmeOrder) -> Result<PathBuf, OperationError> {
        let main = order.domains.main().as_str();
        let folder = if order.key.is_ecc() { format!("{main}_ecc") } else { main.to_owned() };
        let made = self.paths.state.join("certificates").join(order.certificate.to_string()).join(folder);
        let chain = read_pem(&made.join("fullchain.cer"), "BEGIN CERTIFICATE")?;
        let key = read_pem(&made.join(format!("{main}.key")), "PRIVATE KEY")?;
        let served = self.paths.served(order.certificate);
        fs::create_dir_all(&served).map_err(|e| OperationError::Write(served.clone(), e))?;
        let _ = fs::set_permissions(&served, fs::Permissions::from_mode(0o700));
        write_file(&served.join("key.pem"), &key, 0o600)?;
        write_file(&served.join("fullchain.pem"), &chain, 0o644)?;
        Ok(served.join("fullchain.pem"))
    }

    async fn expiry(&self, chain: &Path) -> Result<i64, OperationError> {
        let path = chain.display().to_string();
        let output =
            zendiloos_exec::run(&self.openssl, ["x509", "-noout", "-enddate", "-in", path.as_str()], QUICK)
                .await?
                .success(&self.openssl)?;
        let text = String::from_utf8(output.stdout).map_err(|_| OperationError::NotUtf8)?;
        not_after(&text)
            .ok_or_else(|| OperationError::Failed(format!("openssl: cannot read {}", text.trim())))
    }

    /// Removes what acme.sh and nginx kept of a certificate.
    pub(crate) fn remove(&self, certificate: u32) -> Result<Response, OperationError> {
        let kept = self.paths.state.join("certificates").join(certificate.to_string());
        for folder in [kept, self.paths.served(certificate)] {
            match fs::remove_dir_all(&folder) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(OperationError::Write(folder, error)),
            }
        }
        Ok(Response::Done)
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
                 mkdir -p \"$home/${{main}}_ecc\"\nprintf -- '-----BEGIN CERTIFICATE-----\\n' > \"$home/${{main}}_ecc/fullchain.cer\"\n\
                 printf -- '-----BEGIN PRIVATE KEY-----\\n' > \"$home/${{main}}_ecc/$main.key\"\necho Cert success.\n",
                fail = fail.display()
            ),
        );
        executable(
            &dir.join("systemd-run"),
            &format!(
                "#!/bin/sh\nfor a; do case \"$a\" in --property=EnvironmentFile=*) cp \"${{a##*=}}\" {seen};; esac; done\n\
                 while [ \"$1\" != {script} ]; do shift; done\nexec \"$@\"\n",
                seen = dir.join("seen.env").display(),
                script = paths.script.display()
            ),
        );
        executable(&dir.join("openssl"), "#!/bin/sh\necho 'notAfter=Feb  6 12:30:15 2027 GMT'\n");
        let tools =
            Tools { systemd_run: dir.join("systemd-run"), openssl: dir.join("openssl"), ..Tools::default() };
        Acme::new(&tools, paths).unwrap()
    }

    #[tokio::test]
    async fn a_certificate_is_issued_copied_for_nginx_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let acme = acme(dir.path());
        assert_eq!(acme.state(), Response::AcmeState { installed: true });
        let dns = order(ionos("s3cr3t"), AcmeKey::Ec256);
        assert_eq!(acme.issue(&dns).await.unwrap(), Response::CertificateIssued { not_after: 1_801_917_015 });
        let served = dir.path().join("web/certificates/3");
        assert!(fs::read_to_string(served.join("fullchain.pem")).unwrap().contains("BEGIN CERTIFICATE"));
        let mode = fs::metadata(served.join("key.pem")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // The credentials were there while it ran, and are gone now.
        assert!(fs::read_to_string(dir.path().join("seen.env")).unwrap().contains("IONOS_SECRET=\"s3cr3t\""));
        assert!(!dir.path().join("run/3.env").exists());

        fs::write(dir.path().join("fail"), "").unwrap();
        let failed = acme.issue(&dns).await.unwrap_err();
        assert_eq!(failed.to_string(), "acme.sh: Error add txt for domain");
        assert!(!dir.path().join("run/3.env").exists());

        let unknown = AcmeChallenge::Dns {
            api: DnsApi::parse("dns_nope").unwrap(),
            credentials: DnsCredentials::default(),
        };
        assert!(
            acme.issue(&order(unknown, AcmeKey::Ec256)).await.unwrap_err().to_string().contains("no DNS API")
        );

        assert_eq!(acme.remove(3).unwrap(), Response::Done);
        assert!(!served.exists() && !dir.path().join("state/certificates/3").exists());
        assert_eq!(acme.remove(3).unwrap(), Response::Done, "nothing left is fine");
        fs::remove_file(dir.path().join("share/acme.sh")).unwrap();
        assert_eq!(acme.state(), Response::AcmeState { installed: false });
        assert!(acme.issue(&dns).await.unwrap_err().to_string().contains("not installed"));
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
        assert!(acme.install(&order(AcmeChallenge::Http, AcmeKey::Ec256)).is_err());
        fs::remove_file(made.join("fullchain.cer")).unwrap();
        fs::write(made.join("fullchain.cer"), "not a certificate").unwrap();
        assert!(acme.install(&order(AcmeChallenge::Http, AcmeKey::Ec256)).is_err());
    }
}
