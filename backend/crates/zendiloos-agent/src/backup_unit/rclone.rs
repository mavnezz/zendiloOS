//! The unit's work with rclone (docs/architecture.md, 4.14): backups to
//! Google Drive and OneDrive, and wherever a task's folder is encrypted
//! with rclone's crypt (in a folder of the NAS, a cloud, or on an SSH
//! server through SFTP).
//!
//! rclone's remotes are set for it alone, by variables of its own
//! (`RCLONE_CONFIG_<REMOTE>_<OPTION>`), and a cloud's token by a config
//! file in the unit's private `/tmp` (0600), where rclone writes it again
//! when it refreshes it: it is read back after every call, and the newest
//! goes back to zendiloosd. The crypt's password and salt are obscured as
//! rclone keeps them, by `rclone obscure -` reading them on standard
//! input. None of it reaches a command line.
//!
//! `base` is the place: a cloud's folder, an SFTP server's; a folder of
//! the NAS is opened beneath its root, as for rsync, and rclone is given
//! `/proc/self/fd/<n>`. With a crypt, `dest` is rclone's crypt over the
//! task's folder there: the names and contents of everything in it, the
//! versions' names too, are encrypted. rclone copies no links: it leaves
//! them out (`--skip-links`).
//!
//! Versions are complete copies, as rclone has no hard links: in a cloud
//! the newest is first copied on the server, so only what changed is
//! uploaded, but each takes its full space.

use std::collections::BTreeSet;
use std::os::fd::AsFd;

use rustix::fs::{AtFlags, Gid, Uid};
use serde_json::Value;

use super::*;

/// The place's remote, and the crypt's over the task's folder.
const BASE: &str = "base";
const CRYPT: &str = "dest";
/// What a version's folder holds while it is copied first.
const PREPARING: &str = "preparing";

/// An entry `rclone lsjson` lists.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RcloneEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Size", default)]
    size: i64,
    #[serde(rename = "ModTime", default)]
    modified: String,
    #[serde(rename = "IsDir", default)]
    folder: bool,
}

impl RcloneEntry {
    fn listed(&self) -> Listed {
        Listed { name: self.name.clone(), folder: self.folder }
    }

    /// As `rsync --list-only` writes an entry, which zendiloosd reads.
    fn as_rsync(&self) -> String {
        let mode = if self.folder { "drwxr-xr-x" } else { "-rw-r--r--" };
        let size = if self.folder { 0 } else { self.size.max(0) };
        format!("{mode} {size:>14} {} {}", rsync_time(&self.modified), escaped(&self.name))
    }
}

fn as_rsync(entries: &[RcloneEntry]) -> String {
    entries.iter().map(RcloneEntry::as_rsync).collect::<Vec<_>>().join("\n")
}

/// Days since 1970-01-01 of a date (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let shifted = (month + 9) % 12;
    let of_year = (153 * shifted + 2) / 5 + day - 1;
    let days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + days - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 { shifted + 3 } else { shifted - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Seconds since 1970 of RFC 3339 time as rclone writes it:
/// `2026-11-11T07:00:00.123456789Z`, or with an offset.
fn seconds(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once('T')?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = rest.get(..8)?.split(':').map(str::parse::<i64>);
    let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    let zone = rest.get(8..)?.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone {
        "Z" | "" => 0,
        zone => {
            let sign = match zone.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hours, minutes) = zone[1..].split_once(':')?;
            sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60)
        }
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// `2026/11/11 07:00:00` in UTC, as rsync lists a time.
fn rsync_time(modified: &str) -> String {
    let at = seconds(modified).unwrap_or(0);
    let (year, month, day) = civil_from_days(at.div_euclid(86_400));
    let of_day = at.rem_euclid(86_400);
    format!("{year:04}/{month:02}/{day:02} {:02}:{:02}:{:02}", of_day / 3600, of_day % 3600 / 60, of_day % 60)
}

/// A name as rsync lists it: control characters and `\` as `\#ooo`.
fn escaped(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_control() || c == '\\' {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                out.push_str(&format!("\\#{byte:03o}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A path in rclone's filter rules: its glob characters escaped.
fn glob_escaped(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if "*?[]{}\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `root` and `inside` as one path rclone reads.
fn join(root: &str, inside: &str) -> String {
    match inside {
        "" => root.to_owned(),
        inside if root.ends_with([':', '/']) => format!("{root}{inside}"),
        inside => format!("{root}/{inside}"),
    }
}

/// How rclone's changes are written as rsync's itemized ones.
const NEW: &str = ">f+++++++++";
const REPLACED: &str = ">f.st......";
const DELETING: &str = "*deleting  ";

/// A change rclone logged: what it was, and to what.
fn change(message: &str, object: Option<&str>) -> Option<(&'static str, String)> {
    const CHANGES: [(&str, &str); 6] = [
        ("Copied (new)", NEW),
        ("Copied (Rcat, new)", NEW),
        ("Copied (server-side copy)", NEW),
        ("Copied (replaced existing)", REPLACED),
        ("Deleted", DELETING),
        ("Moved (server-side)", "moved"),
    ];
    let (path, said) = match object {
        Some(object) => (object.to_owned(), message.strip_prefix(&format!("{object}: ")).unwrap_or(message)),
        // Older rclone writes the object before the message.
        None => CHANGES.iter().find_map(|(words, _)| {
            message.find(&format!(": {words}")).map(|at| (message[..at].to_owned(), &message[at + 2..]))
        })?,
    };
    let (_, kind) = CHANGES.iter().find(|(words, _)| said.starts_with(words))?;
    (!path.is_empty()).then_some((*kind, path))
}

/// How rclone's lines (its JSON log, `lsjson`'s entries) are heard.
#[derive(Default)]
struct RcloneHeard {
    stats: Stats,
    transfers: Vec<String>,
    transfers_total: u64,
    /// Files copied or replaced: whose owners a restore sets.
    written: Vec<String>,
    /// What a mirror moved to its kept changes.
    moved: Vec<String>,
    entries: Vec<RcloneEntry>,
    errors: Vec<String>,
    complaints: Vec<String>,
    percent: Option<u8>,
    changed: bool,
}

impl RcloneHeard {
    fn hear(&mut self, line: &str) {
        let trimmed = line.trim().trim_end_matches(',');
        if trimmed.is_empty() || trimmed == "[" || trimmed == "]" {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            self.complain(trimmed);
            return;
        };
        if value.get("Name").is_some() && value.get("IsDir").is_some() {
            if self.entries.len() < LISTED
                && let Ok(entry) = serde_json::from_value::<RcloneEntry>(value)
            {
                self.entries.push(entry);
            }
            return;
        }
        if let Some(stats) = value.get("stats") {
            let number = |key: &str| stats.get(key).and_then(Value::as_u64).unwrap_or(0);
            self.stats = Stats { files: number("transfers"), bytes: number("bytes") };
            if let Some(share) = number("bytes").saturating_mul(100).checked_div(number("totalBytes")) {
                let percent = u8::try_from(share.min(100)).unwrap_or(100);
                if self.percent != Some(percent) {
                    self.percent = Some(percent);
                    self.changed = true;
                }
            }
            return;
        }
        let level = value.get("level").and_then(Value::as_str).unwrap_or_default();
        let message = value.get("msg").and_then(Value::as_str).unwrap_or_default().trim();
        let object = value.get("object").and_then(Value::as_str).filter(|o| !o.is_empty());
        if let Some((kind, path)) = change(message, object) {
            match kind {
                "moved" => self.moved.push(path),
                kind => {
                    if kind != DELETING {
                        self.written.push(path.clone());
                    }
                    self.itemize(kind, &path);
                }
            }
            return;
        }
        let said = match object {
            Some(object) if !message.starts_with(object) => format!("{object}: {message}"),
            _ => message.to_owned(),
        };
        if matches!(level, "error" | "critical" | "fatal") {
            self.errors.push(said.clone());
        }
        if !matches!(level, "info" | "debug") {
            self.complain(&said);
        }
    }

    fn itemize(&mut self, kind: &str, path: &str) {
        self.transfers_total += 1;
        if self.transfers.len() < TRANSFERS_KEPT {
            self.transfers.push(format!("{kind} {path}"));
        }
    }

    /// What a mirror moved to its kept changes and did not copy anew was
    /// deleted from it.
    fn deletions_kept(&mut self) {
        let written: BTreeSet<&str> = self.written.iter().map(String::as_str).collect();
        let deleted: Vec<String> =
            self.moved.iter().filter(|p| !written.contains(p.as_str())).cloned().collect();
        for path in deleted {
            self.itemize(DELETING, &path);
        }
    }

    fn complain(&mut self, line: &str) {
        if self.complaints.len() == 64 {
            self.complaints.remove(0);
        }
        self.complaints.push(line.to_owned());
    }

    fn reason(&self) -> String {
        let said = if self.errors.is_empty() { &self.complaints } else { &self.errors };
        said.iter().rev().take(4).rev().cloned().collect::<Vec<_>>().join(" ")
    }

    fn missing(&self) -> bool {
        self.complaints.iter().any(|line| line.contains("directory not found"))
    }

    fn host_key_changed(&self) -> bool {
        self.complaints.iter().any(|line| {
            line.contains("knownhosts: key mismatch") || line.contains("knownhosts: key is unknown")
        })
    }

    /// Complete but for files that vanished or changed while read.
    fn partial(&self) -> bool {
        !self.errors.is_empty()
            && self.errors.iter().all(|error| {
                error.contains("source file is being updated")
                    || error.contains("no such file or directory")
                    || error.contains("there were IO errors")
                    || error.contains("not deleting")
            })
    }
}

/// rclone as a unit's work calls it: its remotes, its config file, the
/// folder worked on.
struct Rclone {
    program: Program,
    config: PathBuf,
    cache: PathBuf,
    variables: Vec<(String, String)>,
    /// The folder worked on: `dest:` (the crypt), `base:<path>` or
    /// `/proc/self/fd/<n>`.
    root: String,
    /// The same folder as stored, not decrypted.
    plain: String,
    encrypted: bool,
    cloud: bool,
    /// A cloud's token as given, and as rclone wrote it last.
    given: Option<String>,
    token: Option<String>,
}

fn base(option: &str, value: impl Into<String>) -> (String, String) {
    (format!("RCLONE_CONFIG_BASE_{option}"), value.into())
}

fn crypt(option: &str, value: impl Into<String>) -> (String, String) {
    (format!("RCLONE_CONFIG_DEST_{option}"), value.into())
}

impl Rclone {
    /// rclone for the folder `at` (a folder of the NAS already `opened`).
    async fn new(unit: &Unit<'_>, at: &End, opened: Option<&Opened>) -> Result<Self, Fail> {
        let program = Program::new(&unit.job.rclone).map_err(|e| failed(format!("rclone: {e}")))?;
        let home = unit.scratch.0.display().to_string();
        let mut variables = vec![("HOME".to_owned(), home), ("TZ".to_owned(), "UTC".to_owned())];
        let mut config = String::new();
        let mut given = None;
        let plain = match at {
            End::Local { .. } => {
                let opened = opened.ok_or_else(|| failed("the folder is not open"))?;
                opened.arg("").trim_end_matches('/').to_owned()
            }
            End::Cloud { cloud, path } => {
                let access = unit
                    .secrets
                    .cloud
                    .as_ref()
                    .ok_or_else(|| refused("the cloud account's sign-in is missing"))?;
                match cloud {
                    BackupCloud::GoogleDrive => {
                        variables.push(base("TYPE", "drive"));
                        // Only what rclone made: the backups, nothing else of the drive.
                        variables.push(base("SCOPE", "drive.file"));
                    }
                    BackupCloud::OneDrive => variables.push(base("TYPE", "onedrive")),
                }
                if let Some(id) = &access.client_id {
                    variables.push(base("CLIENT_ID", id.as_str()));
                }
                if let Some(secret) = &access.client_secret {
                    variables.push(base("CLIENT_SECRET", secret.expose()));
                }
                if let Some(id) = &access.drive_id {
                    variables.push(base("DRIVE_ID", id.as_str()));
                }
                if let Some(kind) = &access.drive_type {
                    variables.push(base("DRIVE_TYPE", kind.as_str()));
                }
                let token = access.token.expose().to_owned();
                config = format!("[{BASE}]\ntoken = {token}\n");
                given = Some(token);
                format!("{BASE}:{}", path.as_str())
            }
            End::Ssh { host, port, user, path, .. } => {
                let known = unit.known_hosts(at)?;
                variables.extend([
                    base("TYPE", "sftp"),
                    base("HOST", host.as_str()),
                    base("PORT", port.to_string()),
                    base("USER", user.as_str()),
                    base("KEY_FILE", unit.job.key.display().to_string()),
                    base("KNOWN_HOSTS_FILE", known.display().to_string()),
                    // No commands of its own on the server: only SFTP.
                    base("SHELL_TYPE", "none"),
                    base("DISABLE_HASHCHECK", "true"),
                    base("MD5SUM_COMMAND", "none"),
                    base("SHA1SUM_COMMAND", "none"),
                ]);
                format!("{BASE}:{}", path.as_str())
            }
            End::Daemon { .. } => {
                return Err(refused(
                    "an rsync daemon is reached with rsync alone: a backup to it is not encrypted",
                ));
            }
        };
        let config = unit.scratch.once("rclone.conf", &config)?;
        let cache = unit.scratch.folder("rclone-cache")?;
        let (root, encrypted) = match &unit.secrets.crypt {
            Some(secrets) => {
                variables.extend([
                    crypt("TYPE", "crypt"),
                    crypt("REMOTE", plain.clone()),
                    crypt("FILENAME_ENCRYPTION", "standard"),
                    crypt("DIRECTORY_NAME_ENCRYPTION", "true"),
                    crypt("PASSWORD", obscure(&program, secrets.password.expose(), &variables).await?),
                ]);
                if let Some(salt) = &secrets.salt {
                    variables.push(crypt("PASSWORD2", obscure(&program, salt.expose(), &variables).await?));
                }
                (format!("{CRYPT}:"), true)
            }
            None => (plain.clone(), false),
        };
        let cloud = given.is_some();
        Ok(Self {
            program,
            config,
            cache,
            variables,
            root,
            plain,
            encrypted,
            cloud,
            token: given.clone(),
            given,
        })
    }

    /// The folder `inside` of the one worked on.
    fn at(&self, inside: &str) -> String {
        join(&self.root, inside)
    }

    /// Calls rclone with `args`, its lines to `heard`, its progress to
    /// `out` as `step`; reads its token back after.
    async fn call(
        &mut self,
        args: &[String],
        extra: &[(&str, &str)],
        heard: &mut RcloneHeard,
        out: &mut (dyn Write + Send),
        step: &str,
    ) -> Result<Ended, Fail> {
        let mut all = args.to_vec();
        all.extend([
            format!("--config={}", self.config.display()),
            format!("--cache-dir={}", self.cache.display()),
            "--use-json-log".to_owned(),
            "--ask-password=false".to_owned(),
            "--retries=3".to_owned(),
            "--low-level-retries=10".to_owned(),
            "--contimeout=30s".to_owned(),
            "--timeout=15m".to_owned(),
        ]);
        let mut variables = self.variables.clone();
        variables.extend(extra.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())));
        let surroundings = Surroundings { variables, directory: None };
        // The unit's own time limit ends a run that takes too long.
        let limits = Limits { timeout: std::time::Duration::from_secs(7 * 24 * 3600), max_stdout: 0 };
        let mut last_told: Option<std::time::Instant> = None;
        let ran = zendiloos_exec::run_lines(&self.program, &all, &surroundings, limits, &mut |line| {
            heard.hear(line);
            if heard.changed
                && last_told.is_none_or(|told| told.elapsed() >= std::time::Duration::from_secs(1))
            {
                heard.changed = false;
                last_told = Some(std::time::Instant::now());
                if let Some(percent) = heard.percent {
                    say(out, &UnitSays::Progress { step: step.to_owned(), percent });
                }
            }
        })
        .await
        .map_err(|e| failed(e.to_string()));
        self.reread();
        let output = ran?;
        match output.status.code() {
            Some(0) => Ok(Ended::Done),
            _ if heard.host_key_changed() => Err(Fail { message: heard.reason(), kind: FailKind::HostKey }),
            Some(_) if heard.partial() => Ok(Ended::Partial(heard.reason())),
            code => {
                let reason = heard.reason();
                let reason = if reason.is_empty() {
                    output.stderr.trim().lines().last().unwrap_or_default().to_owned()
                } else {
                    reason
                };
                Err(failed(format!(
                    "rclone ended with {}: {reason}",
                    code.map_or("a signal".to_owned(), |c| c.to_string())
                )))
            }
        }
    }

    /// The token rclone wrote into its config file, if it refreshed it.
    fn reread(&mut self) {
        if !self.cloud {
            return;
        }
        let Ok(text) = std::fs::read_to_string(&self.config) else { return };
        let mut section = "";
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line;
            } else if section == "[base]"
                && let Some(value) =
                    line.strip_prefix("token").map(str::trim_start).and_then(|v| v.strip_prefix('='))
            {
                let value = value.trim();
                if !value.is_empty() && !value.chars().any(char::is_control) {
                    self.token = Some(value.to_owned());
                }
            }
        }
    }

    fn refreshed(&self) -> Option<String> {
        if self.token == self.given { None } else { self.token.clone() }
    }

    /// The folder `inside` of the one worked on (decrypted; or `plain`,
    /// as stored): its entries, or that it is missing.
    async fn list(&mut self, inside: &str, plain: bool) -> Result<(Vec<RcloneEntry>, bool), Fail> {
        let at = join(if plain { &self.plain } else { &self.root }, inside);
        let args = vec!["lsjson".to_owned(), "--no-mimetype".to_owned(), at];
        let mut heard = RcloneHeard::default();
        let called = self.call(&args, &[], &mut heard, &mut std::io::sink(), "listing").await;
        if heard.missing() && !matches!(called, Ok(Ended::Done)) {
            return Ok((Vec::new(), true));
        }
        match called? {
            Ended::Done => Ok((heard.entries, false)),
            Ended::Partial(reason) => Err(failed(reason)),
        }
    }

    /// Runs `args` and nothing told: what it changed is not the run's.
    async fn quietly(&mut self, args: Vec<String>, extra: &[(&str, &str)]) -> Result<RcloneHeard, Fail> {
        let mut heard = RcloneHeard::default();
        match self.call(&args, extra, &mut heard, &mut std::io::sink(), PREPARING).await? {
            Ended::Done => Ok(heard),
            Ended::Partial(reason) => Err(failed(reason)),
        }
    }

    /// The marker, through the crypt if there is one.
    async fn mark(&mut self, unit: &Unit<'_>) -> Result<(), Fail> {
        let file = unit.scratch.once("marker", MARKER_TEXT)?;
        let args = vec!["copyto".to_owned(), file.display().to_string(), self.at(MARKER)];
        self.quietly(args, &[]).await.map(|_| ())
    }
}

/// `secret` as rclone keeps a password in its config: `rclone obscure -`
/// reads it on standard input.
async fn obscure(program: &Program, secret: &str, variables: &[(String, String)]) -> Result<String, Fail> {
    let surroundings = Surroundings {
        variables: variables.iter().filter(|(name, _)| name == "HOME" || name == "TZ").cloned().collect(),
        directory: None,
    };
    let limits = Limits { timeout: std::time::Duration::from_secs(60), max_stdout: 64 * 1024 };
    let input = format!("{secret}\n");
    let output =
        zendiloos_exec::run_with(program, ["obscure", "-"], Some(input.as_bytes()), &surroundings, limits)
            .await
            .map_err(|e| failed(e.to_string()))?;
    let obscured = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || obscured.is_empty() || obscured.contains(char::is_whitespace) {
        return Err(failed(format!("rclone cannot obscure the crypt's password: {}", output.stderr.trim())));
    }
    Ok(obscured)
}

/// rclone's options for what a run leaves out, and its bandwidth: each
/// pattern for files and folders alike, as rsync reads it.
fn filters(run: &RunJob) -> Vec<String> {
    let mut patterns = vec![".zendiloos-upload-*".to_owned()];
    if !run.hidden {
        patterns.push(".*".into());
    }
    patterns.extend(run.excludes.iter().map(ToString::to_string));
    let mut args = Vec::new();
    for pattern in patterns {
        match pattern.strip_suffix('/') {
            Some(folder) => args.push(format!("--exclude={folder}/**")),
            None => {
                args.push(format!("--exclude={pattern}"));
                args.push(format!("--exclude={pattern}/**"));
            }
        }
    }
    if run.bwlimit_kib > 0 {
        args.push(format!("--bwlimit={}K", run.bwlimit_kib));
    }
    args
}

/// What every copy of rclone's is told.
fn copying() -> Vec<String> {
    ["--log-level=INFO", "--stats=2s", "--create-empty-src-dirs", "--skip-links"].map(str::to_owned).into()
}

/// Opens a folder of the NAS for rclone (`Some(None)` for a place
/// elsewhere); `None` if it is not there.
fn open_local(end: &End) -> Result<Option<Option<Opened>>, Fail> {
    match end {
        End::Local { root, path } => open_beneath(root, path, false).map(|opened| opened.map(Some)),
        _ => Ok(Some(None)),
    }
}

impl Unit<'_> {
    pub(super) async fn rclone_list(&self, at: &End, inside: &SharePath) -> Result<Report, Fail> {
        let Some(opened) = open_local(at)? else {
            return Ok(Report { missing: true, ..Report::default() });
        };
        let mut rclone = Rclone::new(self, at, opened.as_ref()).await?;
        let (entries, missing) = rclone.list(inside.as_str(), false).await?;
        Ok(Report { listing: as_rsync(&entries), missing, token: rclone.refreshed(), ..Report::default() })
    }

    pub(super) async fn rclone_run(
        &self,
        run: &RunJob,
        out: &mut (dyn Write + Send),
    ) -> Result<Report, Fail> {
        let End::Local { root, path } = &run.source else {
            return Err(refused("an encrypted backup reads a folder of a share"));
        };
        let source = open_beneath(root, path, false)?
            .ok_or_else(|| refused(format!("the source {} is not there", path.as_str())))?;
        let folder = run.folder.as_str();
        // The task's folder: opened in the target on the NAS, named elsewhere.
        let (target_opened, task_opened, task_end) = match &run.target {
            End::Local { root, path } => {
                let target = open_beneath(root, path, false)?
                    .ok_or_else(|| refused(format!("the target {} is not there", path.as_str())))?;
                let task = open_in(&target, folder, true)?
                    .ok_or_else(|| failed("the task's folder cannot be made"))?;
                (Some(target), Some(task), run.target.join(folder))
            }
            other => (None, None, other.join(folder)),
        };
        let mut rclone = Rclone::new(self, &task_end, task_opened.as_ref()).await?;
        let (entries, _) = rclone.list("", false).await?;
        let listed: Vec<Listed> = entries.iter().map(RcloneEntry::listed).collect();
        let marked = listed.iter().any(|e| e.name == MARKER && !e.folder);
        if !marked {
            let foreign = if rclone.encrypted {
                // What the crypt cannot decrypt is not this task's.
                let (stored, _) = rclone.list("", true).await?;
                stored.len() > entries.len()
            } else {
                false
            };
            if foreign || listed.iter().any(|e| !ours(e)) {
                return Err(refused(format!(
                    "the folder {folder} in the target holds other files (or files encrypted with another password): it is no backup of this task's, and nothing is written into it"
                )));
            }
            rclone.mark(self).await?;
        }
        let stamp = run.stamp.as_str();
        let incomplete = format!("{stamp}{INCOMPLETE}");
        let mut previous: Vec<&str> =
            listed.iter().filter(|e| e.folder && is_stamp(&e.name)).map(|e| e.name.as_str()).collect();
        previous.sort_unstable();
        let mut stale: Vec<&str> = listed
            .iter()
            .filter(|e| {
                e.folder && e.name.strip_suffix(INCOMPLETE).is_some_and(is_stamp) && e.name != incomplete
            })
            .map(|e| e.name.as_str())
            .collect();
        stale.sort_unstable();
        let mut args = vec!["sync".to_owned(), source.arg("").trim_end_matches('/').to_owned()];
        say(out, &UnitSays::Progress { step: "transferring".into(), percent: 0 });
        match run.mode {
            BackupMode::Mirror { keep_changed } => {
                args.push(rclone.at("current"));
                if keep_changed {
                    args.push(format!("--backup-dir={}", rclone.at(&format!("removed/{stamp}"))));
                }
            }
            BackupMode::Versioned => {
                if let Some(cut_off) = stale.last() {
                    // A run cut off before goes on, in its folder.
                    let args = vec!["moveto".to_owned(), rclone.at(cut_off), rclone.at(&incomplete)];
                    rclone.quietly(args, &[]).await?;
                } else if let (true, Some(newest)) = (rclone.cloud, previous.last()) {
                    // Copied on the server: only what changed is uploaded.
                    let args = vec![
                        "copy".to_owned(),
                        rclone.at(newest),
                        rclone.at(&incomplete),
                        "--create-empty-src-dirs".to_owned(),
                    ];
                    rclone.quietly(args, &[]).await?;
                }
                args.push(rclone.at(&incomplete));
            }
        }
        args.extend(copying());
        args.push("--delete-excluded".into());
        args.extend(filters(run));
        let mut heard = RcloneHeard::default();
        let ended = rclone.call(&args, &[], &mut heard, out, "transferring").await?;
        heard.deletions_kept();
        if run.mode == BackupMode::Versioned {
            say(out, &UnitSays::Progress { step: "finishing".into(), percent: 100 });
            let args = vec!["moveto".to_owned(), rclone.at(&incomplete), rclone.at(stamp)];
            rclone.quietly(args, &[]).await?;
        }
        if let Some(task) = &task_opened {
            // A drive pulled out after "done" must not lose the backup.
            rustix::fs::syncfs(&task.fd)
                .map_err(|e| failed(format!("the target cannot be written out: {e}")))?;
        }
        let (entries, _) = rclone.list("", false).await?;
        drop((target_opened, task_opened, source));
        Ok(Report {
            files: heard.stats.files,
            bytes: heard.stats.bytes,
            partial: match ended {
                Ended::Done => None,
                Ended::Partial(reason) => Some(reason),
            },
            transfers: heard.transfers,
            transfers_total: heard.transfers_total,
            listing: as_rsync(&entries),
            missing: false,
            token: rclone.refreshed(),
        })
    }

    /// Removes `names` from the task's folder `at`, for good: not into a
    /// cloud's bin, where they would take space on.
    pub(super) async fn rclone_prune(&self, at: &End, names: &[BackupPruned]) -> Result<Report, Fail> {
        if names.is_empty() {
            return Ok(Report::default());
        }
        let Some(opened) = open_local(at)? else { return Ok(Report::default()) };
        let mut rclone = Rclone::new(self, at, opened.as_ref()).await?;
        let extra = [("RCLONE_CONFIG_BASE_USE_TRASH", "false"), ("RCLONE_CONFIG_BASE_HARD_DELETE", "true")];
        for name in names {
            let args = vec!["purge".to_owned(), rclone.at(name.as_str())];
            let mut heard = RcloneHeard::default();
            match rclone.call(&args, &extra, &mut heard, &mut std::io::sink(), "finishing").await {
                Ok(_) => {}
                Err(_) if heard.missing() => {}
                Err(fail) => return Err(fail),
            }
        }
        Ok(Report { token: rclone.refreshed(), ..Report::default() })
    }

    pub(super) async fn rclone_restore(
        &self,
        restore: &RestoreJob,
        out: &mut (dyn Write + Send),
    ) -> Result<Report, Fail> {
        let End::Local { root, path } = &restore.to else {
            return Err(refused("a restore goes into a folder of the NAS"));
        };
        let Some(from_opened) = open_local(&restore.from)? else {
            return Err(refused("the backup to restore from is not there"));
        };
        let to = open_beneath(root, path, true)?
            .ok_or_else(|| failed("the folder to restore into cannot be made"))?;
        let mut rclone = Rclone::new(self, &restore.from, from_opened.as_ref()).await?;
        let mut args = vec![
            "copy".to_owned(),
            rclone.at(restore.within.as_str()),
            to.arg("").trim_end_matches('/').to_owned(),
        ];
        args.extend(copying());
        if restore.existing == BackupExisting::Skip {
            args.push("--ignore-existing".into());
        }
        if !restore.paths.is_empty() {
            let mut rules = String::new();
            for path in &restore.paths {
                let path = glob_escaped(path.as_str());
                rules.push_str(&format!("+ /{path}\n+ /{path}/**\n"));
            }
            rules.push_str("- **\n");
            let file = self.scratch.file("restore-filters", &rules)?;
            args.push(format!("--filter-from={}", file.display()));
        }
        say(out, &UnitSays::Progress { step: "restoring".into(), percent: 0 });
        let mut heard = RcloneHeard::default();
        let ended = rclone.call(&args, &[], &mut heard, out, "restoring").await?;
        if restore.into_share && !to.plain {
            let stat = rustix::fs::fstat(&to.fd).map_err(|e| failed(e.to_string()))?;
            own(&to, &heard.written, stat.st_uid, stat.st_gid)?;
        }
        rustix::fs::syncfs(&to.fd)
            .map_err(|e| failed(format!("what was restored cannot be written out: {e}")))?;
        Ok(Report {
            files: heard.stats.files,
            bytes: heard.stats.bytes,
            partial: match ended {
                Ended::Done => None,
                Ended::Partial(reason) => Some(reason),
            },
            token: rclone.refreshed(),
            ..Report::default()
        })
    }
}

/// Gives what a restore wrote into a share's folder the folder's owner and
/// group, as the explorer's copies have (rclone writes as root), and the
/// folders it made on the way; the share's ACL they inherit. Never through
/// a link.
fn own(to: &Opened, written: &[String], uid: u32, gid: u32) -> Result<(), Fail> {
    let directory = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let (owner, group) = (Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)));
    let mut made: BTreeSet<String> = BTreeSet::new();
    for path in written {
        let Ok(path) = SharePath::parse(path) else { continue };
        let names: Vec<&str> = path.names().collect();
        let Some((file, folders)) = names.split_last() else { continue };
        let mut current: Option<OwnedFd> = None;
        let mut walked = String::new();
        let mut reached = true;
        for name in folders {
            let parent = current.as_ref().map_or_else(|| to.fd.as_fd(), AsFd::as_fd);
            let next = match rustix::fs::openat2(parent, *name, directory, Mode::empty(), BENEATH) {
                Ok(next) => next,
                Err(Errno::NOENT) => {
                    reached = false;
                    break;
                }
                Err(e) => return Err(failed(format!("{name}: {e}"))),
            };
            walked.push_str(name);
            walked.push('/');
            if made.insert(walked.clone()) {
                let stat = rustix::fs::fstat(&next).map_err(|e| failed(e.to_string()))?;
                if stat.st_uid == 0 {
                    rustix::fs::fchown(&next, owner, group)
                        .map_err(|e| failed(format!("{name}: its owner: {e}")))?;
                }
            }
            current = Some(next);
        }
        if !reached {
            continue;
        }
        let parent = current.as_ref().map_or_else(|| to.fd.as_fd(), AsFd::as_fd);
        match rustix::fs::chownat(parent, *file, owner, group, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(e) => return Err(failed(format!("{file}: its owner: {e}"))),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
