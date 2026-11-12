//! The web interface's ports and certificate.
//!
//! nginx's site includes two files zendiloOS writes in
//! `/etc/zendiloos/web`: `http.conf`, the plain HTTP server, which points
//! to HTTPS and answers HTTP challenges (or nothing, when plain HTTP is
//! off), and `https.conf`, the HTTPS server's `listen` lines and
//! certificate. A change keeps the files it replaces beside them
//! (`….previous`), is checked with `nginx -t` and nginx reloaded. Unless it
//! is confirmed in time, the previous files return: by a timer in the
//! agent, and, should the agent or the machine restart first, when the
//! agent starts. So settings the browser cannot reach undo themselves.
//! Settings confirmed before, which zendiloosd puts in force again at its
//! start, stay at once.
//!
//! `nginx -t` runs in a transient unit: it opens nginx's log and its
//! temporary folders, which the agent's sandbox has read-only.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{Mutex, MutexGuard};
use tokio::task::JoinHandle;
use zendiloos_agent_proto::{ConfirmWithin, HostName, PendingWebServer, Response, WebServerConfig};
use zendiloos_exec::{Limits, Program, Surroundings};

use crate::config::Tools;
use crate::operations::OperationError;

const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };
const HEADER: &str =
    "# Written by zendiloOS; changes made here are replaced. Change it in the web interface.\n";
/// A `.previous` file when there was no file before.
const NO_FILE: &str = "# zendiloos: there was no file\n";
const FILES: [&str; 2] = ["http.conf", "https.conf"];

/// Where the settings are written; other than the real places in tests.
#[derive(Debug, Clone)]
pub struct WebServerPaths {
    /// `/etc/zendiloos/web`.
    pub directory: PathBuf,
    /// Issued certificates as nginx reads them: `<n>/fullchain.pem`,
    /// `<n>/key.pem`.
    pub certificates: PathBuf,
    /// The machine's own, self-signed certificate and its key.
    pub own_certificate: PathBuf,
    pub own_key: PathBuf,
    /// Files acme.sh writes for HTTP challenges.
    pub webroot: PathBuf,
}

impl Default for WebServerPaths {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("/etc/zendiloos/web"),
            certificates: PathBuf::from("/etc/zendiloos/web/certificates"),
            own_certificate: PathBuf::from("/etc/zendiloos/tls/zendiloos.crt"),
            own_key: PathBuf::from("/etc/zendiloos/tls/zendiloos.key"),
            webroot: PathBuf::from("/var/lib/zendiloos-acme/webroot"),
        }
    }
}

impl WebServerPaths {
    /// The certificate and key nginx is to serve.
    fn certificate(&self, config: &WebServerConfig) -> (PathBuf, PathBuf) {
        match config.certificate {
            Some(n) => {
                let folder = self.certificates.join(n.to_string());
                (folder.join("fullchain.pem"), folder.join("key.pem"))
            }
            None => (self.own_certificate.clone(), self.own_key.clone()),
        }
    }
}

/// The plain HTTP server: to HTTPS, with HTTP challenges answered.
pub(crate) fn http_file(config: &WebServerConfig, paths: &WebServerPaths) -> String {
    let Some(port) = config.http else {
        return format!("{HEADER}# Plain HTTP is not served.\n");
    };
    let https = config.https.get();
    let to = if https == 443 { String::new() } else { format!(":{https}") };
    format!(
        "{HEADER}server {{\n    listen {port} default_server;\n    listen [::]:{port} default_server;\n    \
         server_tokens off;\n\n    \
         # Let's Encrypt's HTTP challenges, answered with what acme.sh wrote: as\n    \
         # text whatever the name, and never through a link its account made.\n    \
         location ^~ /.well-known/acme-challenge/ {{\n        root {webroot};\n        \
         types {{ }}\n        default_type text/plain;\n        disable_symlinks on;\n        \
         try_files $uri =404;\n    }}\n\n    \
         # Everything else to HTTPS: the session cookie is HTTPS-only.\n    \
         location / {{\n        return 301 https://$host{to}$request_uri;\n    }}\n}}\n",
        port = port.get(),
        webroot = paths.webroot.display(),
    )
}

/// The HTTPS server's ports and certificate, inside its `server` block.
pub(crate) fn https_file(config: &WebServerConfig, paths: &WebServerPaths) -> String {
    let (certificate, key) = paths.certificate(config);
    format!(
        "{HEADER}listen {port} ssl default_server;\nlisten [::]:{port} ssl default_server;\n\
         ssl_certificate     {};\nssl_certificate_key {};\n",
        certificate.display(),
        key.display(),
        port = config.https.get(),
    )
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[derive(Debug)]
struct Pending {
    revert_at: i64,
    timer: JoinHandle<()>,
}

/// The lock changes of the web server take, held for a change of what
/// nginx reads made elsewhere (a certificate's pair swapped in): no change
/// is applied, goes back or reloads nginx meanwhile.
pub(crate) struct Held<'a> {
    web_server: &'a WebServer,
    _pending: MutexGuard<'a, Option<Pending>>,
}

impl Held<'_> {
    /// `nginx -t`: whether nginx would start with what it reads now.
    pub(crate) async fn check(&self) -> Result<(), OperationError> {
        self.web_server.check().await
    }
}

#[derive(Debug)]
pub(crate) struct WebServer {
    systemctl: Program,
    systemd_run: Program,
    nginx: PathBuf,
    paths: WebServerPaths,
    pending: Mutex<Option<Pending>>,
}

impl WebServer {
    pub(crate) fn new(tools: &Tools, paths: WebServerPaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            systemctl: Program::new(&tools.systemctl)?,
            systemd_run: Program::new(&tools.systemd_run)?,
            nginx: tools.nginx.clone(),
            paths,
            pending: Mutex::new(None),
        })
    }

    fn file(&self, name: &str) -> PathBuf {
        self.paths.directory.join(name)
    }

    fn previous(&self, name: &str) -> PathBuf {
        self.paths.directory.join(format!("{name}.previous"))
    }

    pub(crate) async fn state(&self) -> Response {
        let pending = self.pending.lock().await;
        Response::WebServerState {
            pending: pending.as_ref().map(|p| PendingWebServer { revert_at: p.revert_at }),
        }
    }

    /// `nginx -t`, in a unit of its own; nginx's complaint if it fails.
    async fn check(&self) -> Result<(), OperationError> {
        let nginx = self.nginx.display().to_string();
        let args = [
            "--quiet",
            "--wait",
            "--pipe",
            "--collect",
            "--service-type=exec",
            "--description=zendiloOS: checking nginx's settings",
            "--property=PrivateNetwork=yes",
            "--property=PrivateTmp=yes",
            nginx.as_str(),
            "-t",
            "-q",
        ];
        let output =
            zendiloos_exec::run_combined(&self.systemd_run, args, &Surroundings::default(), LIMITS).await?;
        if output.status.success() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().map(str::trim).rfind(|l| l.contains("[emerg]")).or(text.lines().last());
        Err(OperationError::Failed(line.unwrap_or("nginx -t failed").trim().to_owned()))
    }

    async fn reload(&self) -> Result<(), OperationError> {
        zendiloos_exec::run(&self.systemctl, &["try-reload-or-restart", "nginx.service"], LIMITS)
            .await?
            .success(&self.systemctl)?;
        Ok(())
    }

    /// See [`zendiloos_agent_proto::Request::ApplyWebServer`].
    pub(crate) async fn apply(
        self: &Arc<Self>,
        config: &WebServerConfig,
        confirm_within: Option<ConfirmWithin>,
    ) -> Result<Response, OperationError> {
        let (certificate, key) = self.paths.certificate(config);
        if !certificate.is_file() || !key.is_file() {
            return Err(OperationError::Refused("the certificate is not issued".into()));
        }
        let mut pending = self.pending.lock().await;
        if pending.is_some() {
            return Err(OperationError::Conflict);
        }
        let Some(confirm_within) = confirm_within else {
            return self.keep(config).await;
        };
        self.put(config).await?;
        let seconds = confirm_within.get();
        let agent = Arc::clone(self);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(seconds))).await;
            // Held while the old settings return: no change begins meanwhile
            // and has its files put back half way.
            let mut pending = agent.pending.lock().await;
            if pending.take().is_some() {
                tracing::warn!("web server: not confirmed in time, the old settings return");
                agent.restore().await;
            }
        });
        *pending = Some(Pending { revert_at: now() + i64::from(seconds), timer });
        tracing::info!(
            http = ?config.http.map(|p| p.get()),
            https = config.https.get(),
            certificate = ?config.certificate,
            seconds,
            "web server: applied, waiting for confirmation"
        );
        Ok(Response::Done)
    }

    /// Puts settings confirmed before in force again, with `pending` held:
    /// zendiloosd saved them as it confirmed them, and the host went back
    /// to the old ones had zendiloosd stopped before the confirmation came.
    async fn keep(&self, config: &WebServerConfig) -> Result<Response, OperationError> {
        let texts = [http_file(config, &self.paths), https_file(config, &self.paths)];
        let in_force = FILES
            .iter()
            .zip(&texts)
            .all(|(name, text)| fs::read_to_string(self.file(name)).is_ok_and(|now| now == *text));
        if in_force {
            return Ok(Response::Done);
        }
        self.put(config).await?;
        for name in FILES {
            let previous = self.previous(name);
            fs::remove_file(&previous).map_err(|e| OperationError::Write(previous, e))?;
        }
        tracing::warn!(
            http = ?config.http.map(|p| p.get()),
            https = config.https.get(),
            certificate = ?config.certificate,
            "web server: the settings confirmed before are in force again"
        );
        Ok(Response::Done)
    }

    /// Writes the files for `config`, keeping those it replaces beside them,
    /// and has nginx check and take them, with `pending` held: refused, the
    /// files before return at once.
    async fn put(&self, config: &WebServerConfig) -> Result<(), OperationError> {
        let directory = &self.paths.directory;
        fs::create_dir_all(directory).map_err(|e| OperationError::Write(directory.clone(), e))?;
        for name in FILES {
            let file = self.file(name);
            let before = match fs::read_to_string(&file) {
                Ok(content) => content,
                Err(error) if error.kind() == io::ErrorKind::NotFound => NO_FILE.to_owned(),
                Err(error) => return Err(OperationError::Read(file, error)),
            };
            crate::durable::replace(&self.previous(name), before.as_bytes())?;
        }
        let written =
            crate::durable::replace(&self.file("http.conf"), http_file(config, &self.paths).as_bytes())
                .and_then(|()| {
                    crate::durable::replace(
                        &self.file("https.conf"),
                        https_file(config, &self.paths).as_bytes(),
                    )
                });
        let applied = match written {
            Ok(()) => match self.check().await {
                Ok(()) => self.reload().await,
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };
        if let Err(error) = applied {
            // Not even applied: back at once.
            self.restore().await;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) async fn confirm(&self) -> Result<Response, OperationError> {
        let mut pending = self.pending.lock().await;
        let Some(change) = pending.take() else {
            return Err(OperationError::Refused("no change waits for confirmation".into()));
        };
        change.timer.abort();
        for name in FILES {
            let previous = self.previous(name);
            fs::remove_file(&previous).map_err(|e| OperationError::Write(previous, e))?;
        }
        tracing::info!("web server: confirmed");
        Ok(Response::Done)
    }

    /// See [`Held`].
    pub(crate) async fn hold(&self) -> Held<'_> {
        Held { web_server: self, _pending: self.pending.lock().await }
    }

    /// Writes certificates and keys restored from a configuration backup
    /// (`files`: where, what, its mode), under the lock changes take, has
    /// `nginx -t` read them and nginx reload; refused, every file returns
    /// as it was (or goes, where there was none).
    pub(crate) async fn put_restored(&self, files: &[(PathBuf, Vec<u8>, u32)]) -> Result<(), OperationError> {
        use std::os::unix::fs::DirBuilderExt;
        let _pending = self.pending.lock().await;
        let mut before: Vec<(PathBuf, Option<Vec<u8>>, u32)> = Vec::new();
        let written = (|| {
            for (path, content, mode) in files {
                let previous = match fs::read(path) {
                    Ok(bytes) => Some(bytes),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(OperationError::Read(path.clone(), error)),
                };
                if let Some(parent) = path.parent() {
                    fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o755)
                        .create(parent)
                        .map_err(|e| OperationError::Write(parent.to_owned(), e))?;
                }
                before.push((path.clone(), previous, *mode));
                crate::durable::replace_mode(path, content, *mode)?;
            }
            Ok(())
        })();
        let taken = match written {
            Ok(()) => match self.check().await {
                Ok(()) => self.reload().await,
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };
        if let Err(error) = taken {
            for (path, previous, mode) in before.iter().rev() {
                let back = match previous {
                    Some(bytes) => crate::durable::replace_mode(path, bytes, *mode),
                    None => fs::remove_file(path).map_err(|e| OperationError::Write(path.clone(), e)),
                };
                if let Err(back) = back {
                    tracing::error!(%back, path = %path.display(), "cannot put a certificate back");
                }
            }
            return Err(OperationError::Failed(format!(
                "nginx does not take the restored certificates: {error}"
            )));
        }
        Ok(())
    }

    /// nginx reads its certificate again: it was renewed. Not while a
    /// change is applied or goes back.
    pub(crate) async fn reload_certificate(&self) -> Result<Response, OperationError> {
        let _pending = self.pending.lock().await;
        self.check().await?;
        self.reload().await?;
        Ok(Response::Done)
    }

    /// Whether nginx is set to serve the machine's own certificate (or will
    /// be once a change waiting for confirmation goes back).
    fn serves_own(&self) -> bool {
        let own = format!("ssl_certificate     {};", self.paths.own_certificate.display());
        ["https.conf", "https.conf.previous"]
            .iter()
            .any(|name| fs::read_to_string(self.file(name)).is_ok_and(|text| text.contains(&own)))
    }

    /// Makes the machine's own, self-signed certificate anew for `name`
    /// (`<name>`, `<name>.local`, `localhost`, as the package's
    /// `make-certificate` does): the pair is made in a folder of its own
    /// beside the one served (`openssl req`, the key private from the
    /// start), swapped in, and if nginx serves it, checked with `nginx -t`
    /// and nginx reloaded, under the lock changes of the web server take;
    /// refused, the pair before returns. An issued certificate is never
    /// touched. Whether nginx serves the own one.
    pub(crate) async fn renew_own_certificate(
        &self,
        openssl: &Program,
        name: &HostName,
    ) -> Result<bool, OperationError> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        let (certificate, key) = (&self.paths.own_certificate, &self.paths.own_key);
        let folder = certificate.parent().unwrap_or(certificate).to_owned();
        let staged = folder.join("zendiloos-new");
        let write = |path: &Path| {
            let path = path.to_owned();
            move |error: io::Error| OperationError::Write(path, error)
        };
        match fs::remove_dir_all(&staged) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(OperationError::Write(staged, error)),
        }
        fs::create_dir_all(&folder).map_err(write(&folder))?;
        fs::DirBuilder::new().mode(0o700).create(&staged).map_err(write(&staged))?;
        let file_name = |path: &Path| path.file_name().map_or_else(|| "file".into(), ToOwned::to_owned);
        let (new_certificate, new_key) = (staged.join(file_name(certificate)), staged.join(file_name(key)));
        // openssl writes into the file as it is: private before the key is in it.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&new_key)
            .map_err(write(&new_key))?;
        let (subject, names) = (
            format!("/CN={}", name.as_str()),
            format!("subjectAltName=DNS:{0},DNS:{0}.local,DNS:localhost,IP:127.0.0.1", name.as_str()),
        );
        let (key_out, certificate_out) =
            (new_key.display().to_string(), new_certificate.display().to_string());
        let args = [
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-nodes",
            "-days",
            "3650",
            "-subj",
            &subject,
            "-addext",
            &names,
            "-keyout",
            &key_out,
            "-out",
            &certificate_out,
        ];
        let made = zendiloos_exec::run(openssl, args, LIMITS)
            .await
            .map_err(OperationError::from)
            .and_then(|output| output.success(openssl).map(|_| ()).map_err(OperationError::from));
        if let Err(error) = made {
            let _ = fs::remove_dir_all(&staged);
            return Err(error);
        }
        fs::set_permissions(&new_certificate, fs::Permissions::from_mode(0o644))
            .map_err(write(&new_certificate))?;
        // Held from the swap until it stands or is undone: no change of the
        // web server, and no reload, meets the new pair before nginx -t did.
        let _pending = self.pending.lock().await;
        // The certificate first, the key last, as make-certificate does.
        let certificate_swap = swap(&new_certificate, certificate).map_err(write(certificate))?;
        let key_swap = match swap(&new_key, key) {
            Ok(exchanged) => exchanged,
            Err(error) => {
                let _ = unswap(&new_certificate, certificate, certificate_swap);
                return Err(OperationError::Write(key.clone(), error));
            }
        };
        sync_folder(&folder)?;
        let served = self.serves_own();
        if served {
            let taken = match self.check().await {
                Ok(()) => self.reload().await,
                Err(error) => Err(error),
            };
            if let Err(refused) = taken {
                unswap(&new_key, key, key_swap).map_err(write(key))?;
                unswap(&new_certificate, certificate, certificate_swap).map_err(write(certificate))?;
                sync_folder(&folder)?;
                let _ = fs::remove_dir_all(&staged);
                return Err(OperationError::Failed(format!(
                    "nginx does not take the new certificate: {refused}"
                )));
            }
        }
        fs::remove_dir_all(&staged).map_err(write(&staged))?;
        tracing::info!(name = name.as_str(), served, "web server: the machine's own certificate made anew");
        Ok(served)
    }

    /// Puts the previous files back (or removes ours where there were none)
    /// and reloads nginx, with `pending` held. Failures are logged: there is
    /// nobody to tell.
    async fn restore(&self) {
        let mut restored = true;
        for name in FILES {
            let (file, previous) = (self.file(name), self.previous(name));
            let done = match fs::read_to_string(&previous) {
                Ok(content) if content == NO_FILE => fs::remove_file(&file)
                    .or_else(|e| if e.kind() == io::ErrorKind::NotFound { Ok(()) } else { Err(e) }),
                Ok(content) => crate::durable::replace(&file, content.as_bytes()).map_err(io::Error::other),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
            match done.and_then(|()| {
                fs::remove_file(&previous)
                    .or_else(|e| if e.kind() == io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
            }) {
                Ok(()) => {}
                Err(error) => {
                    restored = false;
                    tracing::error!(%error, file = name, "web server: cannot put the old settings back");
                }
            }
        }
        if restored && let Err(error) = self.reload().await {
            tracing::error!(%error, "web server: cannot apply the old settings");
        }
    }

    /// At startup: a change nobody confirmed before the agent stopped goes
    /// back.
    pub(crate) async fn restore_unconfirmed(&self) {
        let _pending = self.pending.lock().await;
        if FILES.iter().any(|name| self.previous(name).exists()) {
            tracing::warn!("web server: a change was not confirmed, the old settings return");
            self.restore().await;
        }
    }
}

/// Puts `new` at `served` in one step, the file there before at `new`:
/// whether there was one.
fn swap(new: &Path, served: &Path) -> io::Result<bool> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    match renameat_with(CWD, new, CWD, served, RenameFlags::EXCHANGE) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::NOENT) => fs::rename(new, served).map(|()| false),
        Err(error) => Err(error.into()),
    }
}

/// Undoes [`swap`]: the file before back at `served`, or none if there
/// was none.
fn unswap(new: &Path, served: &Path, exchanged: bool) -> io::Result<()> {
    if exchanged { swap(new, served).map(|_| ()) } else { fs::remove_file(served) }
}

fn sync_folder(folder: &Path) -> Result<(), OperationError> {
    fs::File::open(folder).and_then(|f| f.sync_all()).map_err(|e| OperationError::Write(folder.to_owned(), e))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use zendiloos_agent_proto::WebPort;

    use super::*;

    fn config(http: Option<u16>, https: u16, certificate: Option<u32>) -> WebServerConfig {
        WebServerConfig {
            http: http.map(|p| WebPort::new(p).unwrap()),
            https: WebPort::new(https).unwrap(),
            certificate,
        }
    }

    #[test]
    fn the_files_say_exactly_the_settings() {
        let paths = WebServerPaths::default();
        let http = http_file(&config(Some(80), 443, None), &paths);
        assert!(http.contains("listen 80 default_server;\n    listen [::]:80 default_server;"), "{http}");
        assert!(http.contains("root /var/lib/zendiloos-acme/webroot;"));
        // What acme.sh's account leaves there is text, and no link is followed.
        assert!(
            http.contains("types { }\n        default_type text/plain;\n        disable_symlinks on;\n"),
            "{http}"
        );
        assert!(http.contains("return 301 https://$host$request_uri;"), "{http}");
        let http = http_file(&config(Some(8080), 8443, None), &paths);
        assert!(
            http.contains("listen 8080 default_server;") && http.contains("https://$host:8443$request_uri")
        );
        assert!(!http_file(&config(None, 443, None), &paths).contains("server"));

        let https = https_file(&config(None, 8443, Some(3)), &paths);
        assert!(
            https.contains("listen 8443 ssl default_server;\nlisten [::]:8443 ssl default_server;\n"),
            "{https}"
        );
        assert!(https.contains("ssl_certificate     /etc/zendiloos/web/certificates/3/fullchain.pem;"));
        assert!(https.contains("ssl_certificate_key /etc/zendiloos/web/certificates/3/key.pem;"));
        let own = https_file(&config(None, 443, None), &paths);
        assert!(own.contains("ssl_certificate     /etc/zendiloos/tls/zendiloos.crt;"), "{own}");
    }

    /// The settings in `dir`; nginx -t fails while `fail` is there, and
    /// systemctl writes what it was asked into `calls`.
    fn web_server(dir: &Path) -> Arc<WebServer> {
        let executable = |path: &Path, text: String| {
            fs::write(path, text).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        };
        let nginx = dir.join("nginx");
        executable(
            &nginx,
            format!(
                "#!/bin/sh\nif [ -f {} ]; then echo 'nginx: [emerg] bind() to 0.0.0.0:8443 failed'; \
                 echo 'nginx: configuration file test failed'; exit 1; fi\n",
                dir.join("fail").display()
            ),
        );
        executable(
            &dir.join("systemd-run"),
            format!("#!/bin/sh\nwhile [ \"$1\" != {} ]; do shift; done\nexec \"$@\"\n", nginx.display()),
        );
        executable(
            &dir.join("systemctl"),
            format!("#!/bin/sh\necho \"$@\" >> {}\n", dir.join("calls").display()),
        );
        let paths = WebServerPaths {
            directory: dir.join("web"),
            certificates: dir.join("web/certificates"),
            own_certificate: dir.join("tls/zendiloos.crt"),
            own_key: dir.join("tls/zendiloos.key"),
            webroot: dir.join("webroot"),
        };
        fs::create_dir_all(dir.join("tls")).unwrap();
        fs::write(&paths.own_certificate, "").unwrap();
        fs::write(&paths.own_key, "").unwrap();
        let tools = Tools {
            systemctl: dir.join("systemctl"),
            systemd_run: dir.join("systemd-run"),
            nginx,
            ..Tools::default()
        };
        Arc::new(WebServer::new(&tools, paths).unwrap())
    }

    fn calls(dir: &Path) -> usize {
        fs::read_to_string(dir.join("calls")).unwrap_or_default().lines().count()
    }

    #[tokio::test]
    async fn a_change_stays_once_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let web = web_server(dir.path());
        let within = ConfirmWithin::seconds(30).ok();
        let refused = web.apply(&config(None, 8443, Some(3)), within).await.unwrap_err();
        assert!(refused.to_string().contains("not issued"), "{refused}");

        assert_eq!(web.apply(&config(Some(80), 8443, None), within).await.unwrap(), Response::Done);
        let Response::WebServerState { pending: Some(pending) } = web.state().await else { panic!() };
        assert!(pending.revert_at > now());
        assert!(matches!(web.apply(&config(None, 443, None), within).await, Err(OperationError::Conflict)));
        assert_eq!(web.confirm().await.unwrap(), Response::Done);
        assert_eq!(web.state().await, Response::WebServerState { pending: None });
        let https = fs::read_to_string(dir.path().join("web/https.conf")).unwrap();
        assert!(https.contains("listen 8443 ssl"), "{https}");
        assert!(!dir.path().join("web/https.conf.previous").exists());
        assert!(web.confirm().await.is_err(), "nothing waits");
        assert_eq!(calls(dir.path()), 1);
    }

    /// Settings zendiloosd confirmed before, which the host went back from
    /// (zendiloosd stopped before the agent heard of the confirmation): put
    /// in force for good, at once; nothing happens while they are.
    #[tokio::test]
    async fn settings_confirmed_before_are_put_in_force_again_for_good() {
        let dir = tempfile::tempdir().unwrap();
        let web = web_server(dir.path());
        let https = || fs::read_to_string(dir.path().join("web/https.conf")).unwrap();
        let previous = |name: &str| dir.path().join(format!("web/{name}.previous")).exists();
        let moved = config(Some(80), 8443, None);
        assert_eq!(web.apply(&moved, None).await.unwrap(), Response::Done);
        assert_eq!(web.state().await, Response::WebServerState { pending: None });
        assert!(https().contains("listen 8443 ssl"), "{}", https());
        assert!(!previous("http.conf") && !previous("https.conf"));
        assert_eq!(calls(dir.path()), 1);
        // In force already: nginx is left alone.
        web.apply(&moved, None).await.unwrap();
        assert_eq!(calls(dir.path()), 1);

        // nginx refuses them: the files before stay.
        fs::write(dir.path().join("fail"), "").unwrap();
        assert!(web.apply(&config(None, 9443, None), None).await.is_err());
        assert!(https().contains("listen 8443 ssl") && !previous("https.conf"));
        fs::remove_file(dir.path().join("fail")).unwrap();
        // Not while a change waits for its confirmation.
        web.apply(&config(None, 443, None), ConfirmWithin::seconds(30).ok()).await.unwrap();
        assert!(matches!(web.apply(&moved, None).await, Err(OperationError::Conflict)));
    }

    #[tokio::test]
    async fn settings_nginx_refuses_go_back_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let web = web_server(dir.path());
        fs::write(dir.path().join("fail"), "").unwrap();
        let failed = web.apply(&config(Some(80), 8443, None), ConfirmWithin::seconds(30).ok()).await;
        assert_eq!(failed.unwrap_err().to_string(), "nginx: [emerg] bind() to 0.0.0.0:8443 failed");
        // There were no files before: there are none now.
        assert!(
            !dir.path().join("web/https.conf").exists()
                && !dir.path().join("web/http.conf.previous").exists()
        );
        assert_eq!(web.state().await, Response::WebServerState { pending: None });
        assert!(web.reload_certificate().await.is_err());
        fs::remove_file(dir.path().join("fail")).unwrap();
        assert_eq!(web.reload_certificate().await.unwrap(), Response::Done);
    }

    #[tokio::test]
    async fn a_change_not_confirmed_goes_back_by_itself_and_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let web = web_server(dir.path());
        fs::create_dir_all(dir.path().join("web")).unwrap();
        fs::write(dir.path().join("web/https.conf"), "listen 443 ssl;\n").unwrap();
        web.apply(&config(None, 8443, None), ConfirmWithin::seconds(30).ok()).await.unwrap();
        // What the timer does when the time is up.
        web.pending.lock().await.take().unwrap().timer.abort();
        web.restore().await;
        assert_eq!(fs::read_to_string(dir.path().join("web/https.conf")).unwrap(), "listen 443 ssl;\n");
        assert!(!dir.path().join("web/http.conf").exists());
        assert_eq!(calls(dir.path()), 2, "reloaded with the new settings, then the old");

        // A restart before the change was confirmed.
        fs::write(dir.path().join("web/https.conf"), "listen 8443 ssl;\n").unwrap();
        fs::write(dir.path().join("web/https.conf.previous"), "listen 443 ssl;\n").unwrap();
        fs::write(dir.path().join("web/http.conf.previous"), NO_FILE).unwrap();
        fs::write(dir.path().join("web/http.conf"), "server {}\n").unwrap();
        web.restore_unconfirmed().await;
        assert_eq!(fs::read_to_string(dir.path().join("web/https.conf")).unwrap(), "listen 443 ssl;\n");
        assert!(
            !dir.path().join("web/http.conf").exists()
                && !dir.path().join("web/https.conf.previous").exists()
        );
    }
}
