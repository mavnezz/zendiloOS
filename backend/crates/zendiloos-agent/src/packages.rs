//! Debian packages through apt, and zendiloOS's own releases
//! (docs/architecture.md, 4.10).
//!
//! apt writes all over the system and fetches over the network; the agent
//! has neither. So it starts a unit of its own, `zendiloos-packages.service`
//! (one at a time), which runs this binary as `run-packages <id> <task>`
//! outside the sandbox. The runner records each step in
//! `/var/lib/zendiloos-packages/run.json`, which the agent reads back; a run
//! outlives a restart of the agent or of zendiloosd, which matters when it
//! installs zendiloOS itself. An update's record goes to `update.json` as
//! well, where the runs after it (a check, a plugin installed) leave it.
//!
//! A release of zendiloOS is installed only after the runner copied its
//! package, `SHA256SUMS` and `SHA256SUMS.minisig` out of the folder
//! zendiloosd downloaded them to, checked the signature with the release key
//! the package brought along (`/usr/share/zendiloos/release-signing.pub`,
//! or one in `/etc/zendiloos`), the package's checksum, name, architecture
//! and version, and only a newer version than the installed one: an old
//! signed release cannot be brought back. Every build is signed with the
//! same key, the development build of each commit too; what the signature
//! is for is its trusted comment, `zendiloos <version>`, which must name
//! the version installed, and a build is taken only on a channel that
//! takes its kind (a development build on the development channel, a test
//! release on the test channel too).

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use zendiloos_agent_proto::{
    PackageReport, PackageRun, PackageRunState, PackageTask, ReleaseChannel, ReleaseInstall, ReleasePackage,
    Response, UpgradablePackage,
};
use zendiloos_exec::{Limits, Program};

use crate::operations::OperationError;
use crate::pools::HostPaths;

/// The unit a run of apt goes in.
const UNIT: &str = "zendiloos-packages.service";
const STATE_FILE: &str = "run.json";
/// The last update's record, also once other runs came after it.
const UPDATE_FILE: &str = "update.json";
const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };
/// Lines of apt's output kept for a failure.
const TAIL: usize = 15;
/// The files of a release beside its package.
const SUMS: &str = "SHA256SUMS";
const SIGNATURE: &str = "SHA256SUMS.minisig";
/// Largest files of a release taken in.
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
const MAX_SMALL: u64 = 64 * 1024;

#[derive(Debug)]
pub(crate) struct Packages {
    systemctl: Program,
    systemd_run: Program,
    paths: HostPaths,
}

impl Packages {
    pub(crate) fn new(systemctl: Program, systemd_run: Program, paths: HostPaths) -> Self {
        Self { systemctl, systemd_run, paths }
    }

    pub(crate) async fn start(
        &self,
        task: PackageTask,
        release: Option<&ReleaseInstall>,
    ) -> Result<Response, OperationError> {
        if release.is_some() && task != PackageTask::Update {
            return Err(OperationError::Refused("only an update installs a release".into()));
        }
        if let Some(release) = release {
            // Checked here too, for a refusal before anything runs; the
            // runner checks its own copy again.
            let folder = tempfile_folder()?;
            let taken = take_release(&self.paths, release, &folder);
            let _ = fs::remove_dir_all(&folder);
            taken.map_err(OperationError::Refused)?;
        }
        if self.active().await {
            return Err(OperationError::Refused("apt is busy with another run".into()));
        }
        let mut bytes = [0u8; 8];
        rustix::rand::getrandom(&mut bytes, rustix::rand::GetRandomFlags::empty())
            .map_err(|e| OperationError::Failed(format!("no random id: {e}")))?;
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let agent = self.paths.agent.to_string_lossy().into_owned();
        let unit = format!("--unit={UNIT}");
        let description = format!("--description=zendiloOS: Debian packages ({})", task.as_str());
        let mut args = vec![
            unit.as_str(),
            description.as_str(),
            "--collect",
            "--quiet",
            "--property=Type=exec",
            agent.as_str(),
            "run-packages",
            id.as_str(),
            task.as_str(),
        ];
        if let Some(release) = release {
            args.extend([release.package.as_str(), release.channel.as_str()]);
        }
        let output = zendiloos_exec::run(&self.systemd_run, &args, LIMITS).await?;
        if !output.status.success() {
            let complaint = output.stderr.trim().to_owned();
            return Err(OperationError::Failed(format!("systemd-run: {complaint}")));
        }
        Ok(Response::PackagesStarted { id })
    }

    pub(crate) async fn state(&self) -> Result<Response, OperationError> {
        let mut run = self.recorded(STATE_FILE)?;
        // Unreadable, it takes nothing from the latest run's record, which
        // the plugins' installs follow too.
        let mut update = self.recorded(UPDATE_FILE).unwrap_or_else(|error| {
            tracing::warn!(%error, "packages: the last update's record cannot be read");
            None
        });
        let unseen = |run: &mut PackageRun| {
            run.state = PackageRunState::Failed;
            run.error = Some("the run stopped before it finished (was the NAS restarted?)".into());
        };
        let going =
            |run: &Option<PackageRun>| run.as_ref().is_some_and(|run| run.state == PackageRunState::Running);
        let active = (going(&run) || going(&update)) && self.active().await;
        let latest = run.as_ref().map(|run| run.id.clone());
        if let Some(run) = run.as_mut().filter(|run| run.state == PackageRunState::Running && !active) {
            unseen(run);
        }
        // An update cut off before another run started is not that run.
        if let Some(update) = update
            .as_mut()
            .filter(|update| update.state == PackageRunState::Running)
            .filter(|update| !active || latest.as_deref() != Some(update.id.as_str()))
        {
            unseen(update);
        }
        Ok(Response::Packages { run, update: update.map(Box::new) })
    }

    /// A run as the runner recorded it in `file`, if it did.
    fn recorded(&self, file: &str) -> Result<Option<PackageRun>, OperationError> {
        let path = self.paths.packages.join(file);
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|_| OperationError::Refused(format!("{} cannot be read", path.display()))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(OperationError::Read(path, error)),
        }
    }

    /// Whether a run of apt goes on now.
    pub(crate) async fn running(&self) -> bool {
        self.active().await
    }

    async fn active(&self) -> bool {
        zendiloos_exec::run(&self.systemctl, ["is-active", "--quiet", UNIT], LIMITS)
            .await
            .is_ok_and(|output| output.status.success())
    }
}

/// A folder of the agent's own in its private `/tmp`, for a first look at a
/// release.
fn tempfile_folder() -> Result<PathBuf, OperationError> {
    let mut bytes = [0u8; 8];
    rustix::rand::getrandom(&mut bytes, rustix::rand::GetRandomFlags::empty())
        .map_err(|e| OperationError::Failed(format!("no random name: {e}")))?;
    let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(std::env::temp_dir().join(format!("zendiloos-release-{name}")))
}

/// `run-packages <id> <task> [<release> <channel>]`, in
/// `zendiloos-packages.service`: does the task, each step recorded.
pub fn run_packages(
    paths: &HostPaths,
    id: &str,
    task: &str,
    release: Option<&str>,
    channel: Option<&str>,
) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{id:?} is no run id"));
    }
    let task = PackageTask::parse(task).ok_or_else(|| format!("{task:?} is no task"))?;
    let release = match (release, channel) {
        (None, None) => None,
        (Some(name), Some(channel)) => Some(ReleaseInstall {
            package: ReleasePackage::parse(name).map_err(|_| format!("{name:?} is no release"))?,
            channel: ReleaseChannel::parse(channel).ok_or_else(|| format!("{channel:?} is no channel"))?,
        }),
        (Some(_), None) => return Err("a release is installed only for a channel".into()),
        (None, Some(_)) => return Err("a channel comes only with a release".into()),
    };
    if release.is_some() && task != PackageTask::Update {
        return Err("only an update installs a release".into());
    }
    // A run started once is never run again under its id, should its unit
    // be started anew: its record stays as it is.
    if fs::read_to_string(paths.packages.join(STATE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<PackageRun>(&text).ok())
        .is_some_and(|recorded| recorded.id == id)
    {
        return Err(format!("the run {id} was started before"));
    }
    let mut run = PackageRun {
        id: id.to_owned(),
        task,
        state: PackageRunState::Running,
        step: if task == PackageTask::Finish { "configure" } else { "lists" }.into(),
        started_at: now(),
        finished_at: None,
        error: None,
        release: release.as_ref().map(|release| release.package.version().to_owned()),
        report: None,
    };
    record(&paths.packages, &run)?;
    let mut report = None;
    let done = steps(paths, task, release.as_ref(), &mut report, |step| {
        run.step = step.to_owned();
        record(&paths.packages, &run)
    });
    run.finished_at = Some(now());
    run.state = if done.is_ok() { PackageRunState::Succeeded } else { PackageRunState::Failed };
    run.error = done.as_ref().err().cloned();
    run.report = report;
    record(&paths.packages, &run)?;
    done
}

/// The steps of `task`; `step` records each as it begins.
fn steps(
    paths: &HostPaths,
    task: PackageTask,
    release: Option<&ReleaseInstall>,
    report: &mut Option<PackageReport>,
    mut step: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    // apt's lists, fetched anew; what an interrupted run left half done is
    // finished first, but for a check, which changes nothing.
    let lists = |step: &mut dyn FnMut(&str) -> Result<(), String>| {
        step("lists")?;
        if task != PackageTask::Check {
            finish_interrupted()?;
        }
        apt(&["update"])
    };
    match task {
        PackageTask::InstallDocker => {
            lists(&mut step)?;
            step("install")?;
            apt(&["install", "--yes", "docker.io", "docker-compose"])
        }
        PackageTask::InstallAcme => {
            lists(&mut step)?;
            step("install")?;
            apt(&["install", "--yes", "acme.sh"])
        }
        PackageTask::Check => {
            lists(&mut step)?;
            *report = Some(host_report()?);
            Ok(())
        }
        PackageTask::Update => {
            lists(&mut step)?;
            step("debian")?;
            apt(&["--with-new-pkgs", "--yes", "upgrade"])?;
            if let Some(release) = release {
                step("zendiloos")?;
                install_release(paths, release)?;
            }
            step("modules")?;
            let checked = host_report()?;
            *report = Some(checked.clone());
            zfs_checked(&checked)
        }
        PackageTask::Finish => {
            // Nothing fetched: what was unpacked is configured, and apt
            // completes what is missing from the lists it has.
            step("configure")?;
            finish_interrupted()?;
            step("repair")?;
            apt(&["--fix-broken", "--yes", "install"])?;
            step("modules")?;
            let checked = host_report()?;
            *report = Some(checked.clone());
            if checked.interrupted {
                return Err("dpkg still lists packages it did not finish (`dpkg --audit` names them)".into());
            }
            zfs_checked(&checked)
        }
    }
}

/// Fails the run if the ZFS module is missing for the newest kernel.
fn zfs_checked(report: &PackageReport) -> Result<(), String> {
    if report.zfs_ready {
        return Ok(());
    }
    let kernel = report.newest_kernel.clone().unwrap_or_default();
    Err(format!(
        "the ZFS module is missing for kernel {kernel}: do not restart; the pools would not be found. \
         `dkms autoinstall -k {kernel}` builds it (the kernel's headers are needed)."
    ))
}

/// Copies the release out of the download folder, checks it and installs it.
fn install_release(paths: &HostPaths, install: &ReleaseInstall) -> Result<(), String> {
    let folder = paths.packages.join("release");
    let package = take_release(paths, install, &folder)?;
    let release = &install.package;
    let field = |name: &str| -> Result<String, String> {
        let output = Command::new("/usr/bin/dpkg-deb")
            .args(["--field"])
            .arg(&package)
            .arg(name)
            .output()
            .map_err(|e| format!("dpkg-deb: {e}"))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    if field("Package")? != "zendiloos" {
        return Err("the release's package is not zendiloos".into());
    }
    if field("Version")? != release.version() {
        return Err("the release's package has another version than its name says".into());
    }
    let arch = run_text("/usr/bin/dpkg", &["--print-architecture"])?;
    if field("Architecture")? != arch {
        return Err(format!("the release's package is not for {arch}"));
    }
    let installed = run_text("/usr/bin/dpkg-query", &["--show", "--showformat=${Version}", "zendiloos"])
        .unwrap_or_default();
    if !installed.is_empty() && !newer(release.version(), &installed)? {
        return Err(format!("{} is not newer than {installed}, which is installed", release.version()));
    }
    apt(&["install", "--yes", &package.to_string_lossy()])
}

/// Copies the release's three files from the download folder into `folder`
/// (made anew, root's only), never through a link, and checks that the
/// channel takes the build, the signature, that it was made for this
/// version and the package's checksum. Returns the copied package.
fn take_release(paths: &HostPaths, install: &ReleaseInstall, folder: &Path) -> Result<PathBuf, String> {
    let release = &install.package;
    let (kind, channel) = (release.kind(), install.channel);
    if !channel.takes(kind) {
        let build = match kind {
            ReleaseChannel::Stable => "a release",
            ReleaseChannel::Test => "a test release",
            ReleaseChannel::Development => "a development build",
        };
        return Err(format!(
            "{} is {build}, which the {} channel does not take",
            release.version(),
            channel.as_str()
        ));
    }
    let key = release_key(paths)?;
    let _ = fs::remove_dir_all(folder);
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    let spool = &paths.update_spool;
    let sums = copy_plain(&spool.join(SUMS), &folder.join(SUMS), MAX_SMALL)?;
    let signature = copy_plain(&spool.join(SIGNATURE), &folder.join(SIGNATURE), MAX_SMALL)?;
    let package = folder.join(release.as_str());
    copy_plain(&spool.join(release.as_str()), &package, MAX_PACKAGE)?;
    let signature = minisign_verify::Signature::decode(&String::from_utf8_lossy(&signature))
        .map_err(|_| "the release's signature cannot be read".to_owned())?;
    key.verify(&sums, &signature, false)
        .map_err(|_| "the release's signature does not match its key".to_owned())?;
    // Signed too: which build the signature was made for (CI signs with
    // `-t "zendiloos <version>"`); it must be the one installed.
    let meant = format!("zendiloos {}", release.version());
    if signature.trusted_comment() != meant {
        return Err(format!(
            "the release's signature is for {:?}, not for {meant:?}",
            signature.trusted_comment()
        ));
    }
    let expected = String::from_utf8_lossy(&sums)
        .lines()
        .find_map(|line| {
            let (sum, name) = line.split_once(char::is_whitespace)?;
            (name.trim_start().trim_start_matches('*') == release.as_str()).then(|| sum.to_ascii_lowercase())
        })
        .ok_or_else(|| format!("{SUMS} does not list {}", release.as_str()))?;
    let mut hasher = Sha256::new();
    let mut file = fs::File::open(&package).map_err(|e| format!("{}: {e}", package.display()))?;
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(|e| format!("{}: {e}", package.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err(format!("the checksum of {} does not match", release.as_str()));
    }
    Ok(package)
}

/// The key releases are signed with: one in `/etc/zendiloos` replaces the
/// one the package brought along.
fn release_key(paths: &HostPaths) -> Result<minisign_verify::PublicKey, String> {
    let file = paths
        .release_keys
        .iter()
        .find(|path| path.exists())
        .ok_or("there is no key to check releases with; this build of zendiloOS takes no updates")?;
    let text = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    minisign_verify::PublicKey::decode(&text)
        .or_else(|_| minisign_verify::PublicKey::from_base64(text.trim()))
        .map_err(|_| format!("{} holds no minisign key", file.display()))
}

/// Copies a regular file, never through a link, of at most `max` bytes;
/// returns the content of small ones.
fn copy_plain(from: &Path, to: &Path, max: u64) -> Result<Vec<u8>, String> {
    // Not through a link, and a FIFO put in the download folder opens at
    // once (and is refused below) rather than waiting for a writer.
    let mut source = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(from)
        .map_err(|e| format!("{}: {e}", from.display()))?;
    let metadata = source.metadata().map_err(|e| format!("{}: {e}", from.display()))?;
    if !metadata.is_file() || metadata.len() > max {
        return Err(format!("{} is no file of the release", from.display()));
    }
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)
        .map_err(|e| format!("{}: {e}", to.display()))?;
    let mut kept = Vec::new();
    let copied = std::io::copy(&mut (&mut source).take(max + 1), &mut target)
        .map_err(|e| format!("{}: {e}", to.display()))?;
    if copied > max {
        return Err(format!("{} is too large", from.display()));
    }
    if max <= MAX_SMALL {
        kept = fs::read(to).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    Ok(kept)
}

/// What an upgrade would bring now, which kernel runs, which is newest,
/// whether ZFS is built for it, and whether dpkg left packages half done.
fn host_report() -> Result<PackageReport, String> {
    let interrupted = interrupted()?;
    let output = Command::new("/usr/bin/apt-get")
        .args(["--simulate", "--quiet", "-o", "Debug::NoLocking=1", "--with-new-pkgs", "upgrade"])
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("apt-get cannot be started: {e}"))?;
    // With packages half installed apt may see nothing to upgrade, or
    // refuse to plan: finishing the update comes first either way.
    let upgradable = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).lines().filter_map(upgradable).collect()
    } else if interrupted {
        Vec::new()
    } else {
        return Err(format!("apt-get --simulate upgrade failed ({})", output.status));
    };
    let running_kernel = rustix::system::uname().release().to_string_lossy().into_owned();
    let newest_kernel = newest_kernel(Path::new("/lib/modules"))?;
    let zfs_ready =
        newest_kernel.as_deref().is_none_or(|kernel| has_zfs(&Path::new("/lib/modules").join(kernel)));
    Ok(PackageReport { upgradable, running_kernel, newest_kernel, zfs_ready, interrupted })
}

/// Whether dpkg left packages unpacked, half installed or half configured,
/// or with triggers waiting: `dpkg --audit` lists them (an update cut off,
/// by a power cut while DKMS built ZFS, say), and says nothing otherwise.
fn interrupted() -> Result<bool, String> {
    let output = Command::new("/usr/bin/dpkg")
        .arg("--audit")
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("dpkg cannot be started: {e}"))?;
    Ok(audit_lists_something(&output.stdout))
}

/// What `dpkg --audit` printed names packages: it prints nothing when all
/// is well, whatever its exit status says.
fn audit_lists_something(printed: &[u8]) -> bool {
    !String::from_utf8_lossy(printed).trim().is_empty()
}

/// One line of `apt-get --simulate`: `Inst openssl [3.5.1-1] (3.5.4-1
/// Debian-Security:13/stable-security [amd64])`.
fn upgradable(line: &str) -> Option<UpgradablePackage> {
    let rest = line.strip_prefix("Inst ")?;
    let (name, rest) = rest.split_once(' ')?;
    let (from, rest) = match rest.strip_prefix('[') {
        Some(rest) => {
            let (from, rest) = rest.split_once("] ")?;
            (Some(from.to_owned()), rest)
        }
        None => (None, rest),
    };
    let inside = rest.strip_prefix('(')?;
    let to = inside.split_whitespace().next()?.to_owned();
    let security = inside.to_ascii_lowercase().contains("security");
    Some(UpgradablePackage { name: name.to_owned(), from, to, security })
}

/// The newest kernel with modules, by dpkg's order of versions.
fn newest_kernel(modules: &Path) -> Result<Option<String>, String> {
    let entries = match fs::read_dir(modules) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", modules.display())),
    };
    let mut newest: Option<String> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().join("modules.dep").exists() && !entry.path().join("kernel").is_dir() {
            continue;
        }
        newest = match newest {
            Some(current) if !newer(&name, &current)? => Some(current),
            _ => Some(name),
        };
    }
    Ok(newest)
}

/// A `zfs.ko*` somewhere below the kernel's modules (DKMS puts it in
/// `updates/dkms`).
fn has_zfs(kernel: &Path) -> bool {
    fn walk(folder: &Path, depth: usize) -> bool {
        let Ok(entries) = fs::read_dir(folder) else { return false };
        entries.flatten().any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "zfs.ko" || name.starts_with("zfs.ko.") {
                return true;
            }
            depth > 0 && entry.file_type().is_ok_and(|kind| kind.is_dir()) && walk(&entry.path(), depth - 1)
        })
    }
    walk(kernel, 6)
}

/// `a` is a newer version than `b`, as dpkg orders them.
fn newer(a: &str, b: &str) -> Result<bool, String> {
    let status = Command::new("/usr/bin/dpkg")
        .args(["--compare-versions", a, "gt", b])
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("dpkg: {e}"))?;
    Ok(status.success())
}

fn run_text(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!("{program} failed ({})", output.status))
    }
}

/// `apt-get` without questions, keeping changed configuration files, waiting
/// up to ten minutes for another apt to finish. Its output goes to the
/// journal; the last lines of a failure into the error.
///
/// needrestart (which `docker.io` recommends) stays out of it: it would
/// restart this very unit once an update replaces this binary, and Docker
/// with every app after a library's update. Services restart as their own
/// packages say, as on a Debian without needrestart.
/// Finishes what an interrupted apt or dpkg left half done (a restart in
/// the middle of an update): until then every run of apt fails with "dpkg
/// was interrupted". Nothing to do is quick.
fn finish_interrupted() -> Result<(), String> {
    debian("/usr/bin/dpkg", &["--force-confdef", "--force-confold", "--configure", "-a"])
}

fn apt(args: &[&str]) -> Result<(), String> {
    let mut all = vec!["--quiet", "-o", "DPkg::Lock::Timeout=600"];
    all.extend(["-o", "Dpkg::Options::=--force-confdef", "-o", "Dpkg::Options::=--force-confold"]);
    all.extend_from_slice(args);
    debian("/usr/bin/apt-get", &all)
}

/// Runs apt-get or dpkg without asking anything; prints what it said.
fn debian(program: &str, args: &[&str]) -> Result<(), String> {
    let name = Path::new(program).file_name().and_then(|name| name.to_str()).unwrap_or(program);
    // `apt-get upgrade`: the first word that is no option nor its value.
    let action =
        args.iter().find(|arg| !arg.starts_with('-') && !arg.contains('=')).map(|arg| format!(" {arg}"));
    let output = Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LANG", "C.UTF-8")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("APT_LISTCHANGES_FRONTEND", "none")
        .env("NEEDRESTART_SUSPEND", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{name} cannot be started: {e}"))?;
    let text =
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{text}");
    if output.status.success() {
        return Ok(());
    }
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(TAIL)..].join("\n");
    Err(format!("{name}{} failed ({}):\n{tail}", action.unwrap_or_default(), output.status))
}

/// Writes the run's state, readable by everyone, replacing the last; an
/// update's also as the last update's.
fn record(folder: &Path, run: &PackageRun) -> Result<(), String> {
    match fs::DirBuilder::new().mode(0o755).create(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("{}: {error}", folder.display())),
    }
    let text = serde_json::to_string_pretty(run).map_err(|e| e.to_string())?;
    let files: &[&str] = if run.task.updates() { &[STATE_FILE, UPDATE_FILE] } else { &[STATE_FILE] };
    for name in files {
        let path = folder.join(name);
        let temporary = folder.join(format!("{name}.new"));
        let written = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o644)
                .open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &path)
        })();
        written.map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs().try_into().unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(task: PackageTask) -> PackageRun {
        PackageRun {
            id: "ab12".into(),
            task,
            state: PackageRunState::Running,
            step: "install".into(),
            started_at: 1,
            finished_at: None,
            error: None,
            release: None,
            report: None,
        }
    }

    #[test]
    fn a_fifo_in_the_download_folder_is_refused_at_once() {
        let folder = tempfile::tempdir().unwrap();
        let fifo = folder.path().join("zendiloos_9.9.9_amd64.deb");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        let target = folder.path().join("copy");
        let (done, waited) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(copy_plain(&fifo, &target, 1024));
        });
        let copied = waited.recv_timeout(std::time::Duration::from_secs(5)).expect("it waited for a writer");
        assert!(copied.unwrap_err().contains("no file of the release"));
    }
    #[test]
    fn a_run_is_recorded_for_the_agent_to_read() {
        let folder = tempfile::tempdir().unwrap();
        let run = run(PackageTask::InstallDocker);
        record(folder.path(), &run).unwrap();
        let read: PackageRun =
            serde_json::from_str(&fs::read_to_string(folder.path().join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(read, run);
    }

    #[test]
    fn a_run_is_never_run_twice() {
        let folder = tempfile::tempdir().unwrap();
        let paths = HostPaths { packages: folder.path().to_owned(), ..HostPaths::default() };
        let mut recorded = run(PackageTask::Update);
        recorded.state = PackageRunState::Succeeded;
        record(folder.path(), &recorded).unwrap();
        let again = run_packages(&paths, "ab12", "update", None, None).unwrap_err();
        assert!(again.contains("started before"), "{again}");
        let kept: PackageRun =
            serde_json::from_str(&fs::read_to_string(folder.path().join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(kept.state, PackageRunState::Succeeded);
    }

    #[test]
    fn only_hex_ids_known_tasks_and_release_names_run() {
        let paths =
            HostPaths { packages: std::env::temp_dir().join("zendiloos-never"), ..HostPaths::default() };
        let run = |task, release, channel| run_packages(&paths, "ab12", task, release, channel).unwrap_err();
        let name = Some("zendiloos_1.0_amd64.deb");
        assert!(
            run_packages(&paths, "../x", "install-docker", None, None).unwrap_err().contains("no run id")
        );
        assert!(run("rm-rf", None, None).contains("no task"));
        assert!(run("update", Some("../etc/shadow"), Some("stable")).contains("no release"));
        assert!(run("install-docker", name, Some("stable")).contains("only an update"));
        assert!(run("update", name, Some("nightly")).contains("no channel"));
        assert!(run("update", name, None).contains("only for a channel"));
        assert!(run("update", None, Some("stable")).contains("only with a release"));
    }

    #[test]
    fn apt_tells_what_an_upgrade_brings() {
        let lines = "NOTE: This is only a simulation!\n\
            Inst openssl [3.5.1-1] (3.5.4-1~deb13u1 Debian-Security:13/stable-security [amd64])\n\
            Inst linux-image-6.12.57+deb13-amd64 (6.12.57-1 Debian:13.2/stable [amd64])\n\
            Conf openssl (3.5.4-1~deb13u1 Debian-Security:13/stable-security [amd64])\n";
        let found: Vec<UpgradablePackage> = lines.lines().filter_map(upgradable).collect();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].from.as_deref(), Some("3.5.1-1"));
        assert_eq!(found[0].to, "3.5.4-1~deb13u1");
        assert!(found[0].security);
        assert_eq!(found[1].name, "linux-image-6.12.57+deb13-amd64");
        assert_eq!(found[1].from, None);
        assert!(!found[1].security);
    }

    #[test]
    fn zfs_is_found_where_dkms_puts_it() {
        let kernel = tempfile::tempdir().unwrap();
        assert!(!has_zfs(kernel.path()));
        fs::create_dir_all(kernel.path().join("updates/dkms")).unwrap();
        fs::write(kernel.path().join("updates/dkms/zfs.ko.xz"), "").unwrap();
        assert!(has_zfs(kernel.path()));
    }

    #[test]
    fn dpkg_tells_of_an_update_it_did_not_finish() {
        let half = "The following packages are only half configured, probably due to problems\n\
            configuring them the first time.  The configuration should be retried using\n\
            dpkg --configure <package> or the configure menu option in dselect:\n \
            zfs-dkms             OpenZFS filesystem kernel modules for Linux\n";
        assert!(audit_lists_something(half.as_bytes()));
        assert!(!audit_lists_something(b""));
        assert!(!audit_lists_something(b"\n"));
        assert_eq!(fs::metadata("/usr/bin/dpkg").is_ok(), interrupted().is_ok(), "dpkg answers where it is");
    }

    #[test]
    fn finishing_an_update_is_kept_as_the_last_update_and_installs_no_release() {
        let folder = tempfile::tempdir().unwrap();
        let finish = run(PackageTask::Finish);
        record(folder.path(), &finish).unwrap();
        let kept: PackageRun =
            serde_json::from_str(&fs::read_to_string(folder.path().join(UPDATE_FILE)).unwrap()).unwrap();
        assert_eq!(kept, finish);
        let paths = HostPaths { packages: folder.path().join("never"), ..HostPaths::default() };
        let release = run_packages(&paths, "ab12", "finish", Some("zendiloos_1.0_amd64.deb"), Some("stable"));
        assert!(release.unwrap_err().contains("only an update"));
    }

    /// A key made with `minisign -G -W` (its secret one is gone), and
    /// `SHA256SUMS` of a "package" signed with it.
    const KEY: &str = "untrusted comment: minisign public key DF2398FD1421641E\n\
        RWQeZCEU/Zgj30ypMHVOUazGHOS4zrR2comO1Cr6nNACcycOZJl0Qv3z\n";
    const PACKAGE: &str = "test package\n";
    const SIGNED_SUMS: &str =
        "d400a577a056c0530d6acfc0e0a2f937be15323bffe02dfc81a8ca66b332a907  zendiloos_9.9.9_amd64.deb\n";
    const SIGNED: &str = "untrusted comment: signature from minisign secret key\n\
        RUQeZCEU/Zgj3w/shOlDbRzCkd75l/5mJ9ZiScpj4yMyq5thsyekWQS5toBgDunkEebNhxIsqz0ubaxSW+AdqscpXf13Q3Q6oAI=\n\
        trusted comment: zendiloos 9.9.9\n\
        xRAvEXYdiCSNSWJHQ2JmtSHjKY61Uz+J7Bn22qhUpJXTEAKzlSUEBlv/lwtADWtkFb5kPh2dk+6Xo2IKSl13BQ==\n";

    /// Another key whose secret one is gone, made for signatures with other
    /// trusted comments: `SIGNED_SUMS` signed for 9.9.8, and the checksums
    /// of a development build signed for that build.
    const OTHER_KEY: &str = "untrusted comment: minisign public key 64334BBC6AD71F0D\n\
        RWQNH9dqvEszZBP8zLvEJaXEL3AzndjV/5wJLCtU4Vj2UFjaa1O4/Ijf\n";
    const SIGNED_FOR_ANOTHER: &str = "untrusted comment: signature from minisign secret key\n\
        RUQNH9dqvEszZE28tIZ5pPGptSWi2crgbDvT7WB7Thh69qZIOVhs/ObxnBOPBQDb1tdVxSKJ3tEmRIOMDVZbbw8VrDN1osBgTgY=\n\
        trusted comment: zendiloos 9.9.8\n\
        YbW99j5ug79YvQtA1AW2G/TQbQed51Ds7DtPqOjG2xqJSoF7CE3gmmL7htem2+RVGJvMANrEtQ+jjGzn8tTKAA==\n";
    const DEVELOPMENT: &str = "zendiloos_9.9.9+dev.20261109120000.gabc1234_amd64.deb";
    const DEVELOPMENT_SUMS: &str = "d400a577a056c0530d6acfc0e0a2f937be15323bffe02dfc81a8ca66b332a907  \
        zendiloos_9.9.9+dev.20261109120000.gabc1234_amd64.deb\n";
    const DEVELOPMENT_SIGNED: &str = "untrusted comment: signature from minisign secret key\n\
        RUQNH9dqvEszZK9ejYeLNlNRnqqEZ1yc0ckau96IKa9JSiTMaDej4eANdET8QVDZZaL7FQRSy00nu51qlL0O8Ha929tiD0ZJgAs=\n\
        trusted comment: zendiloos 9.9.9+dev.20261109120000.gabc1234\n\
        lalLpd3mpRs+QBDw828tI7T7nNDkJI4ZNpfRG2Onmx4fKZRJ9G79/H00JUf5X+YoylMs2mTcvXSJs9e46NPvCw==\n";

    fn install(name: &str, channel: ReleaseChannel) -> ReleaseInstall {
        ReleaseInstall { package: ReleasePackage::parse(name).unwrap(), channel }
    }

    fn stable(name: &str) -> ReleaseInstall {
        install(name, ReleaseChannel::Stable)
    }

    #[test]
    fn a_signature_counts_only_for_the_build_it_names_on_a_channel_that_takes_it() {
        let root = tempfile::tempdir().unwrap();
        let spool = root.path().join("spool");
        fs::create_dir_all(&spool).unwrap();
        let key = root.path().join("release.pub");
        fs::write(&key, OTHER_KEY).unwrap();
        let paths =
            HostPaths { update_spool: spool.clone(), release_keys: vec![key], ..HostPaths::default() };
        let taken = root.path().join("taken");
        let put = |name: &str, content: &str| {
            let _ = fs::remove_file(spool.join(name));
            fs::write(spool.join(name), content).unwrap();
        };

        // A good signature of checksums that list 9.9.9, made for 9.9.8.
        let release = stable("zendiloos_9.9.9_amd64.deb");
        put(release.package.as_str(), PACKAGE);
        put(SUMS, SIGNED_SUMS);
        put(SIGNATURE, SIGNED_FOR_ANOTHER);
        let refused = take_release(&paths, &release, &taken).unwrap_err();
        assert!(refused.contains("is for \"zendiloos 9.9.8\""), "{refused}");
        // Nor does it make 9.9.8 of what they list.
        let named = stable("zendiloos_9.9.8_amd64.deb");
        put(named.package.as_str(), PACKAGE);
        assert!(take_release(&paths, &named, &taken).unwrap_err().contains("does not list"));

        // The build of a commit, signed for itself: on the development
        // channel only.
        put(DEVELOPMENT, PACKAGE);
        put(SUMS, DEVELOPMENT_SUMS);
        put(SIGNATURE, DEVELOPMENT_SIGNED);
        for channel in [ReleaseChannel::Stable, ReleaseChannel::Test] {
            let refused = take_release(&paths, &install(DEVELOPMENT, channel), &taken).unwrap_err();
            let says = format!("a development build, which the {} channel does not take", channel.as_str());
            assert!(refused.contains(&says), "{refused}");
        }
        assert!(take_release(&paths, &install(DEVELOPMENT, ReleaseChannel::Development), &taken).is_ok());

        // A test release, refused before anything is read.
        let candidate = take_release(&paths, &stable("zendiloos_9.9.9~rc.1_amd64.deb"), &taken).unwrap_err();
        assert!(candidate.contains("9.9.9~rc.1 is a test release"), "{candidate}");
    }

    #[test]
    fn a_release_is_taken_only_with_its_signature_and_checksum() {
        let root = tempfile::tempdir().unwrap();
        let spool = root.path().join("spool");
        fs::create_dir_all(&spool).unwrap();
        let key = root.path().join("release.pub");
        fs::write(&key, KEY).unwrap();
        let paths = HostPaths {
            update_spool: spool.clone(),
            release_keys: vec![root.path().join("not-there.pub"), key],
            packages: root.path().join("packages"),
            ..HostPaths::default()
        };
        let release = stable("zendiloos_9.9.9_amd64.deb");
        let taken = root.path().join("taken");
        let put = |name: &str, content: &str| {
            let _ = fs::remove_file(spool.join(name));
            fs::write(spool.join(name), content).unwrap();
        };
        put(release.package.as_str(), PACKAGE);
        put(SUMS, SIGNED_SUMS);
        put(SIGNATURE, SIGNED);
        let package = take_release(&paths, &release, &taken).unwrap();
        assert_eq!(fs::read_to_string(package).unwrap(), PACKAGE);

        put(release.package.as_str(), "another package\n");
        let refused = take_release(&paths, &release, &taken).unwrap_err();
        assert!(refused.contains("checksum"), "{refused}");

        put(release.package.as_str(), PACKAGE);
        put(SUMS, &SIGNED_SUMS.replace("d400", "0000"));
        let refused = take_release(&paths, &release, &taken).unwrap_err();
        assert!(refused.contains("signature does not match"), "{refused}");

        // A link in the download folder is never followed.
        fs::remove_file(spool.join(SUMS)).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", spool.join(SUMS)).unwrap();
        let refused = take_release(&paths, &release, &taken).unwrap_err();
        assert!(refused.contains(SUMS), "{refused}");

        let without_key = HostPaths { release_keys: vec![root.path().join("none")], ..paths };
        let refused = take_release(&without_key, &release, &taken).unwrap_err();
        assert!(refused.contains("no key"), "{refused}");
    }

    #[test]
    fn a_release_needs_a_readable_signature_listing_it_and_a_key() {
        let root = tempfile::tempdir().unwrap();
        let spool = root.path().join("spool");
        fs::create_dir_all(&spool).unwrap();
        let key = root.path().join("release.pub");
        let paths = HostPaths {
            update_spool: spool.clone(),
            release_keys: vec![key.clone()],
            ..HostPaths::default()
        };
        let release = stable("zendiloos_9.9.9_amd64.deb");
        let taken = root.path().join("taken");
        fs::write(spool.join(release.package.as_str()), PACKAGE).unwrap();
        fs::write(spool.join(SUMS), SIGNED_SUMS).unwrap();

        fs::write(&key, "no key at all").unwrap();
        assert!(take_release(&paths, &release, &taken).unwrap_err().contains("holds no minisign key"));
        // The key's second line alone is a key too.
        fs::write(&key, KEY.lines().nth(1).unwrap()).unwrap();
        fs::write(spool.join(SIGNATURE), "garbage").unwrap();
        assert!(take_release(&paths, &release, &taken).unwrap_err().contains("cannot be read"));
        fs::write(spool.join(SIGNATURE), SIGNED).unwrap();
        assert!(take_release(&paths, &release, &taken).is_ok());
    }

    #[test]
    fn a_signed_release_that_is_no_zendiloos_package_is_not_installed() {
        let root = tempfile::tempdir().unwrap();
        let spool = root.path().join("spool");
        fs::create_dir_all(&spool).unwrap();
        let key = root.path().join("release.pub");
        fs::write(&key, KEY).unwrap();
        let paths = HostPaths {
            update_spool: spool.clone(),
            release_keys: vec![key],
            packages: root.path().join("packages"),
            ..HostPaths::default()
        };
        let release = stable("zendiloos_9.9.9_amd64.deb");
        fs::write(spool.join(release.package.as_str()), PACKAGE).unwrap();
        fs::write(spool.join(SUMS), SIGNED_SUMS).unwrap();
        fs::write(spool.join(SIGNATURE), SIGNED).unwrap();

        // Signed and listed, but no Debian package at all: dpkg-deb finds no
        // name in it (or is not there to ask), and apt is never run.
        let refused = install_release(&paths, &release).unwrap_err();
        assert!(refused.contains("not zendiloos") || refused.contains("dpkg-deb"), "{refused}");
    }

    #[test]
    fn a_copy_is_bounded_and_never_replaces_anything() {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("from");
        fs::write(&from, "0123456789").unwrap();
        assert!(
            copy_plain(&from, &root.path().join("small"), 4).unwrap_err().contains("no file of the release")
        );
        assert_eq!(copy_plain(&from, &root.path().join("ok"), 64).unwrap(), b"0123456789");
        assert!(copy_plain(&from, &root.path().join("ok"), 64).is_err(), "the target is there already");
        assert!(copy_plain(&root.path().join("missing"), &root.path().join("x"), 64).is_err());
        let big = copy_plain(&from, &root.path().join("big"), MAX_SMALL + 1).unwrap();
        assert!(big.is_empty(), "a package's content is not kept in memory");
    }

    #[test]
    fn the_newest_kernel_with_modules_counts() {
        if !Path::new("/usr/bin/dpkg").exists() {
            eprintln!("skipped: dpkg orders the versions, and it is not here");
            return;
        }
        let modules = tempfile::tempdir().unwrap();
        assert_eq!(newest_kernel(&modules.path().join("missing")).unwrap(), None);
        for (kernel, modules_dep) in
            [("6.12.38+deb13-amd64", true), ("6.12.57+deb13-amd64", true), ("6.99.0-unpacked", false)]
        {
            fs::create_dir_all(modules.path().join(kernel)).unwrap();
            if modules_dep {
                fs::write(modules.path().join(kernel).join("modules.dep"), "").unwrap();
            }
        }
        assert_eq!(newest_kernel(modules.path()).unwrap().as_deref(), Some("6.12.57+deb13-amd64"));
        assert!(newer("1.10", "1.9").unwrap());
        assert!(!newer("1.0~rc1", "1.0").unwrap());
    }

    /// The agent's side, with systemctl and systemd-run as scripts: a file
    /// `active` makes the runner's unit active, `refuse` makes systemd-run
    /// fail.
    fn packages(root: &Path) -> Packages {
        let script = |name: &str, body: &str| {
            let path = root.join(name);
            fs::write(
                &path,
                format!(
                    "#!/bin/sh\necho \"$@\" >> {}\n{body}\n",
                    root.join(name).with_extension("log").display()
                ),
            )
            .unwrap();
            fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            Program::new(&path).unwrap()
        };
        let systemctl = script("systemctl", &format!("[ -f {} ]", root.join("active").display()));
        let systemd_run = script(
            "systemd-run",
            &format!(
                "[ -f {} ] && {{ echo 'unit exists' >&2; exit 1; }}; exit 0",
                root.join("refuse").display()
            ),
        );
        let paths = HostPaths {
            packages: root.join("state"),
            update_spool: root.join("spool"),
            release_keys: vec![root.join("release.pub")],
            ..HostPaths::default()
        };
        Packages::new(systemctl, systemd_run, paths)
    }

    #[tokio::test]
    async fn a_run_starts_in_a_unit_of_its_own_once_at_a_time() {
        let root = tempfile::tempdir().unwrap();
        let packages = packages(root.path());

        let Response::PackagesStarted { id } = packages.start(PackageTask::Check, None).await.unwrap() else {
            panic!()
        };
        assert!(id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
        let asked = fs::read_to_string(root.path().join("systemd-run.log")).unwrap();
        assert!(
            asked.contains(&format!("--unit={UNIT}")) && asked.contains(&format!("run-packages {id} check"))
        );

        fs::write(root.path().join("active"), "").unwrap();
        assert!(packages.running().await);
        let busy = packages.start(PackageTask::Update, None).await.unwrap_err();
        assert!(busy.to_string().contains("busy"), "{busy}");
        fs::remove_file(root.path().join("active")).unwrap();

        fs::write(root.path().join("refuse"), "").unwrap();
        let failed = packages.start(PackageTask::Update, None).await.unwrap_err();
        assert!(failed.to_string().contains("unit exists"), "{failed}");

        let release = stable("zendiloos_9.9.9_amd64.deb");
        let docker = packages.start(PackageTask::InstallDocker, Some(&release)).await.unwrap_err();
        assert!(docker.to_string().contains("only an update"));
        // No key here: the release is refused before anything runs.
        let unchecked = packages.start(PackageTask::Update, Some(&release)).await.unwrap_err();
        assert!(unchecked.to_string().contains("no key"), "{unchecked}");
    }

    #[tokio::test]
    async fn the_runner_is_told_the_channel_it_checks_the_release_against() {
        let root = tempfile::tempdir().unwrap();
        let packages = packages(root.path());
        let spool = root.path().join("spool");
        fs::create_dir_all(&spool).unwrap();
        fs::write(root.path().join("release.pub"), KEY).unwrap();
        fs::write(spool.join("zendiloos_9.9.9_amd64.deb"), PACKAGE).unwrap();
        fs::write(spool.join(SUMS), SIGNED_SUMS).unwrap();
        fs::write(spool.join(SIGNATURE), SIGNED).unwrap();

        let release = install("zendiloos_9.9.9_amd64.deb", ReleaseChannel::Test);
        let Response::PackagesStarted { id } =
            packages.start(PackageTask::Update, Some(&release)).await.unwrap()
        else {
            panic!()
        };
        let asked = fs::read_to_string(root.path().join("systemd-run.log")).unwrap();
        assert!(
            asked.contains(&format!("run-packages {id} update zendiloos_9.9.9_amd64.deb test")),
            "{asked}"
        );
    }

    #[tokio::test]
    async fn a_run_that_stopped_unseen_is_reported_as_failed() {
        let root = tempfile::tempdir().unwrap();
        let packages = packages(root.path());
        assert_eq!(packages.state().await.unwrap(), Response::Packages { run: None, update: None });

        record(&root.path().join("state"), &run(PackageTask::Update)).unwrap();
        fs::write(root.path().join("active"), "").unwrap();
        let Response::Packages { run: Some(going), update: Some(update) } = packages.state().await.unwrap()
        else {
            panic!()
        };
        assert_eq!((going.state, update.state), (PackageRunState::Running, PackageRunState::Running));
        fs::remove_file(root.path().join("active")).unwrap();
        let Response::Packages { run: Some(stopped), update: Some(update) } = packages.state().await.unwrap()
        else {
            panic!()
        };
        assert_eq!((stopped.state, update.state), (PackageRunState::Failed, PackageRunState::Failed));
        assert!(stopped.error.unwrap().contains("restarted"));

        fs::write(root.path().join("state").join(STATE_FILE), "{").unwrap();
        assert!(packages.state().await.unwrap_err().to_string().contains("cannot be read"));
    }

    #[tokio::test]
    async fn an_update_is_still_told_after_a_plugin_was_installed() {
        let root = tempfile::tempdir().unwrap();
        let packages = packages(root.path());
        let state = root.path().join("state");
        let mut update = run(PackageTask::Update);
        update.id = "aa01".into();
        update.state = PackageRunState::Failed;
        update.error = Some("dpkg was interrupted".into());
        record(&state, &update).unwrap();

        // Right after it, Docker is installed, and a check follows.
        let mut docker = run(PackageTask::InstallDocker);
        docker.id = "aa02".into();
        record(&state, &docker).unwrap();
        fs::write(root.path().join("active"), "").unwrap();
        let Response::Packages { run: Some(latest), update: Some(kept) } = packages.state().await.unwrap()
        else {
            panic!()
        };
        assert_eq!((latest.id.as_str(), latest.state), ("aa02", PackageRunState::Running));
        assert_eq!(*kept, update, "the update's record is kept as it was");
        let mut check = run(PackageTask::Check);
        check.id = "aa03".into();
        check.state = PackageRunState::Succeeded;
        record(&state, &check).unwrap();
        let Response::Packages { update: Some(kept), .. } = packages.state().await.unwrap() else { panic!() };
        assert_eq!(kept.id, "aa01");

        // An update cut off unseen, then another run: not the one going on.
        let mut cut_off = run(PackageTask::Update);
        cut_off.id = "aa04".into();
        record(&state, &cut_off).unwrap();
        record(&state, &docker).unwrap();
        let Response::Packages { run: Some(latest), update: Some(kept) } = packages.state().await.unwrap()
        else {
            panic!()
        };
        assert_eq!(latest.state, PackageRunState::Running, "Docker's install goes on");
        assert_eq!((kept.id.as_str(), kept.state), ("aa04", PackageRunState::Failed));

        // A broken record of the update keeps nobody from following a run.
        fs::write(state.join(UPDATE_FILE), "{").unwrap();
        let Response::Packages { run: Some(latest), update: None } = packages.state().await.unwrap() else {
            panic!()
        };
        assert_eq!(latest.id, "aa02");
    }
}
