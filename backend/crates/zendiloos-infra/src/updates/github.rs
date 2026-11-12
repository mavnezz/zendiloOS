//! [`ReleaseSource`] over GitHub's API: the releases of the repository,
//! each with its package for this machine, `SHA256SUMS` and its minisign
//! signature. What is downloaded goes to a folder the agent takes it from;
//! the agent trusts none of it until the signature matches.
//!
//! GitHub's downloads fail now and then for a moment (a `504 Gateway
//! Timeout` for `SHA256SUMS` kept updates from starting), so each file is
//! tried again after a pause when the failure may pass: an answer of 5xx,
//! 408 or 429, a connection that could not be made, broke off or timed
//! out. Only once its tries ran out does the update fail, saying why.

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hyper::Method;
use hyper::header::{ACCEPT, HeaderName};
use serde::Deserialize;
use zendiloos_kernel::Timestamp;
use zendiloos_updates::application::ports::ReleaseSource;
use zendiloos_updates::domain::{Asset, Release, channel_of, package_version};

use crate::https;

const MAX_LIST: usize = 4 * 1024 * 1024;
const MAX_SMALL: usize = 64 * 1024;
const MAX_PACKAGE: usize = 512 * 1024 * 1024;
/// A package over a slow line.
const DOWNLOAD_TIME: Duration = Duration::from_secs(30 * 60);
const SUMS: &str = "SHA256SUMS";
const SIGNATURE: &str = "SHA256SUMS.minisig";
/// The pauses before a file's second, third and fourth try.
const PAUSES: [Duration; 3] = [Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(60)];

pub struct GitHubReleases {
    tls: Arc<rustls::ClientConfig>,
    /// `https://api.github.com`, or a stand-in for tests.
    api: String,
    /// `owner/name`.
    repository: String,
    /// Debian's name of this machine's architecture
    /// ([`zendiloos_agent_proto::ARCHITECTURE`]).
    arch: String,
    /// Where releases are downloaded to (`/var/lib/zendiloos/updates`).
    spool: PathBuf,
    /// [`PAUSES`], shorter in tests.
    pauses: Vec<Duration>,
}

impl GitHubReleases {
    /// For the package of the architecture this build runs on: a release
    /// holds one for each.
    pub fn new(api: &str, repository: &str, spool: PathBuf) -> Self {
        let arch = zendiloos_agent_proto::ARCHITECTURE;
        Self {
            tls: https::client_config(),
            api: api.trim_end_matches('/').to_owned(),
            repository: repository.to_owned(),
            arch: arch.to_owned(),
            spool,
            pauses: PAUSES.to_vec(),
        }
    }

    /// One file of a release, tried again after each of the pauses while
    /// the failure may pass.
    async fn fetch(&self, asset: &Asset, max: usize) -> Result<Vec<u8>, String> {
        let mut pauses = self.pauses.iter();
        let mut tries = 1;
        loop {
            let tried = https::attempt(&self.tls, Method::GET, &asset.address, &[], max, DOWNLOAD_TIME).await;
            let failure = match tried {
                Ok(answer) if answer.status.is_success() => return Ok(answer.body),
                Ok(answer) => https::Failure {
                    reason: format!("the server answered {}", answer.status),
                    transient: https::answered_later(answer.status),
                },
                Err(failure) => failure,
            };
            let Some(pause) = pauses.next().filter(|_| failure.transient) else {
                let times = if tries == 1 { String::new() } else { format!(" (tried {tries} times)") };
                return Err(format!("{}: {}{times}", asset.name, failure.reason));
            };
            tracing::warn!(
                file = asset.name,
                error = failure.reason,
                "updates: download failed, tried again"
            );
            tokio::time::sleep(*pause).await;
            tries += 1;
        }
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    published_at: Option<String>,
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// The releases in GitHub's answer that have everything an update needs.
fn releases(text: &[u8], arch: &str) -> Result<Vec<Release>, String> {
    let listed: Vec<GhRelease> =
        serde_json::from_slice(text).map_err(|e| format!("GitHub answered something unexpected: {e}"))?;
    Ok(listed
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let asset = |pick: &dyn Fn(&str) -> bool| {
                release.assets.iter().find(|asset| pick(&asset.name)).map(|asset| Asset {
                    name: asset.name.clone(),
                    address: asset.browser_download_url.clone(),
                    size: asset.size,
                })
            };
            let package = asset(&|name| package_version(name, arch).is_some())?;
            let version = package_version(&package.name, arch)?.to_owned();
            Some(Release {
                channel: channel_of(&release.tag_name, release.prerelease),
                tag: release.tag_name.clone(),
                version,
                published_at: release
                    .published_at
                    .as_deref()
                    .and_then(|at| at.parse::<jiff::Timestamp>().ok())
                    .map(|at| Timestamp::from_unix_seconds(at.as_second())),
                notes: release.body.clone().unwrap_or_default(),
                sums: asset(&|name| name == SUMS)?,
                signature: asset(&|name| name == SIGNATURE)?,
                package,
            })
        })
        .collect())
}

#[async_trait]
impl ReleaseSource for GitHubReleases {
    async fn releases(&self) -> Result<Vec<Release>, String> {
        let address = format!("{}/repos/{}/releases?per_page=30", self.api, self.repository);
        let headers: Vec<(HeaderName, String)> = vec![
            (ACCEPT, "application/vnd.github+json".to_owned()),
            (HeaderName::from_static("x-github-api-version"), "2022-11-28".to_owned()),
        ];
        let answer = https::send(&self.tls, Method::GET, &address, &headers, MAX_LIST).await?;
        if answer.status == hyper::StatusCode::NOT_FOUND {
            return Err(format!("GitHub does not know the repository {}, or it is private", self.repository));
        }
        if matches!(answer.status, hyper::StatusCode::FORBIDDEN | hyper::StatusCode::TOO_MANY_REQUESTS) {
            return Err(
                "GitHub answers at most 60 requests an hour without an account; try again later".into()
            );
        }
        if !answer.status.is_success() {
            return Err(format!("GitHub answered {}", answer.status));
        }
        releases(&answer.body, &self.arch)
    }

    async fn download(&self, release: &Release) -> Result<(), String> {
        let spool = &self.spool;
        if let Err(error) = std::fs::DirBuilder::new().mode(0o750).recursive(true).create(spool) {
            return Err(format!("{}: {error}", spool.display()));
        }
        // Only this release's files stay.
        if let Ok(entries) = std::fs::read_dir(spool) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        for (asset, max) in
            [(&release.sums, MAX_SMALL), (&release.signature, MAX_SMALL), (&release.package, MAX_PACKAGE)]
        {
            let body = self.fetch(asset, max).await?;
            let path = spool.join(&asset.name);
            let written = (|| {
                use std::io::Write;
                let mut file =
                    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o640).open(&path)?;
                file.write_all(&body)?;
                file.sync_all()
            })();
            written.map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use zendiloos_updates::domain::Channel;

    use super::*;
    use crate::test_https::{Asked, client, response, serve};

    /// Over a server whose answers to each path are given in turn, the last
    /// one for good; pausing for a moment only.
    async fn releases_from(
        answers: Vec<(&'static str, Vec<&'static str>)>,
    ) -> (GitHubReleases, Release, tempfile::TempDir) {
        let answers = Mutex::new(answers);
        let port = serve(move |asked: &Asked| {
            let mut answers = answers.lock().unwrap();
            let Some((_, statuses)) = answers.iter_mut().find(|(path, _)| *path == asked.path) else {
                return response("404 Not Found", &[], b"");
            };
            let status = if statuses.len() > 1 { statuses.remove(0) } else { statuses[0] };
            response(status, &[], if status.starts_with("200") { b"content" } else { b"" })
        })
        .await;
        let spool = tempfile::tempdir().unwrap();
        let mut github = GitHubReleases::new("https://127.0.0.1", "o/r", spool.path().join("updates"));
        (github.tls, github.pauses) = (client(), vec![Duration::from_millis(1); 3]);
        let asset = |name: &str| Asset {
            name: name.to_owned(),
            address: format!("https://127.0.0.1:{port}/{name}"),
            size: 7,
        };
        let release = Release {
            channel: Channel::Stable,
            tag: "v0.5.0".into(),
            version: "0.5.0".into(),
            published_at: None,
            notes: String::new(),
            package: asset("zendiloos_0.5.0_amd64.deb"),
            sums: asset("SHA256SUMS"),
            signature: asset("SHA256SUMS.minisig"),
        };
        (github, release, spool)
    }

    #[tokio::test]
    async fn a_file_github_does_not_give_for_a_moment_is_tried_again() {
        let (github, release, spool) = releases_from(vec![
            ("/SHA256SUMS", vec!["504 Gateway Timeout", "503 Service Unavailable", "200 OK"]),
            ("/SHA256SUMS.minisig", vec!["200 OK"]),
            ("/zendiloos_0.5.0_amd64.deb", vec!["429 Too Many Requests", "200 OK"]),
        ])
        .await;

        github.download(&release).await.unwrap();

        let files = spool.path().join("updates");
        for name in ["SHA256SUMS", "SHA256SUMS.minisig", "zendiloos_0.5.0_amd64.deb"] {
            assert_eq!(std::fs::read(files.join(name)).unwrap(), b"content", "{name}");
        }
    }

    #[tokio::test]
    async fn a_download_fails_once_its_tries_ran_out_or_at_once_when_refused() {
        let (github, release, _spool) =
            releases_from(vec![("/SHA256SUMS", vec!["504 Gateway Timeout"])]).await;
        let failed = github.download(&release).await.unwrap_err();
        assert_eq!(failed, "SHA256SUMS: the server answered 504 Gateway Timeout (tried 4 times)");

        let (github, release, _spool) = releases_from(vec![
            ("/SHA256SUMS", vec!["200 OK"]),
            ("/SHA256SUMS.minisig", vec!["404 Not Found", "200 OK"]),
        ])
        .await;
        let failed = github.download(&release).await.unwrap_err();
        assert_eq!(failed, "SHA256SUMS.minisig: the server answered 404 Not Found", "not tried again");
    }

    #[tokio::test]
    async fn a_server_not_reached_is_tried_again_too() {
        let (mut github, mut release, _spool) = releases_from(vec![]).await;
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        release.sums.address = format!("https://127.0.0.1:{closed}/SHA256SUMS");
        github.pauses = vec![Duration::from_millis(1)];
        let failed = github.download(&release).await.unwrap_err();
        assert!(failed.starts_with("SHA256SUMS: cannot connect to 127.0.0.1"), "{failed}");
        assert!(failed.ends_with("(tried 2 times)"), "{failed}");
    }

    #[test]
    fn githubs_answer_becomes_releases_with_their_files() {
        let answer = br#"[
          {"tag_name":"dev","prerelease":true,"draft":false,"published_at":"2026-11-06T10:00:00Z","body":"Latest main",
           "assets":[
             {"name":"zendiloos_0.2.0+dev.20261106100000.gabc1234_amd64.deb","browser_download_url":"https://github.com/o/r/releases/download/dev/p.deb","size":20000000},
             {"name":"SHA256SUMS","browser_download_url":"https://github.com/o/r/releases/download/dev/SHA256SUMS","size":100},
             {"name":"SHA256SUMS.minisig","browser_download_url":"https://github.com/o/r/releases/download/dev/SHA256SUMS.minisig","size":300}]},
          {"tag_name":"v0.2.0","prerelease":false,"draft":false,"published_at":"2026-11-01T09:00:00Z","body":null,
           "assets":[
             {"name":"zendiloos_0.2.0_arm64.deb","browser_download_url":"https://x/a.deb","size":1},
             {"name":"SHA256SUMS","browser_download_url":"https://x/s","size":1},
             {"name":"SHA256SUMS.minisig","browser_download_url":"https://x/m","size":1}]},
          {"tag_name":"v0.3.0-rc.1","prerelease":true,"draft":true,"assets":[]},
          {"tag_name":"v0.1.0","prerelease":false,"draft":false,"assets":[
             {"name":"zendiloos_0.1.0_amd64.deb","browser_download_url":"https://x/p.deb","size":1}]}
        ]"#;
        let found = releases(answer, "amd64").unwrap();
        assert_eq!(found.len(), 1, "drafts, other machines and releases without a signature are left out");
        let dev = &found[0];
        assert_eq!(dev.channel, Channel::Development);
        assert_eq!(dev.version, "0.2.0+dev.20261106100000.gabc1234");
        assert_eq!(dev.sums.name, "SHA256SUMS");
        assert_eq!(dev.published_at, Some(Timestamp::from_unix_seconds(1_793_959_200)));
        assert_eq!(releases(answer, "arm64").unwrap()[0].tag, "v0.2.0");
        assert!(releases(b"{\"message\":\"Not Found\"}", "amd64").is_err());
    }

    #[test]
    fn each_machine_takes_the_package_of_its_architecture() {
        let answer = br#"[{"tag_name":"v0.6.0","prerelease":false,"draft":false,"assets":[
             {"name":"zendiloos-amd64-netinst.iso","browser_download_url":"https://x/amd64.iso","size":1},
             {"name":"zendiloos-arm64-netinst.iso","browser_download_url":"https://x/arm64.iso","size":1},
             {"name":"zendiloos_0.6.0_amd64.cdx.json","browser_download_url":"https://x/amd64.cdx.json","size":1},
             {"name":"zendiloos_0.6.0_amd64.deb","browser_download_url":"https://x/amd64.deb","size":1},
             {"name":"zendiloos_0.6.0_arm64.cdx.json","browser_download_url":"https://x/arm64.cdx.json","size":1},
             {"name":"zendiloos_0.6.0_arm64.deb","browser_download_url":"https://x/arm64.deb","size":1},
             {"name":"SHA256SUMS","browser_download_url":"https://x/s","size":1},
             {"name":"SHA256SUMS.minisig","browser_download_url":"https://x/m","size":1}]}]"#;
        for arch in ["amd64", "arm64"] {
            let found = releases(answer, arch).unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].version, "0.6.0");
            assert_eq!(found[0].package.name, format!("zendiloos_0.6.0_{arch}.deb"));
            assert_eq!(found[0].package.address, format!("https://x/{arch}.deb"));
        }
        assert!(releases(answer, "riscv64").unwrap().is_empty());
    }
}
