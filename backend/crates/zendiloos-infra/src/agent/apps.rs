//! Apps on the host, through both agents: the system agent prepares,
//! checks and runs them, the files agent makes their folders.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use zendiloos_agent_proto::{
    AgentTransport, AppCompose, AppFolder, AppManifest, AppName, AppSettingValue, AppShareChoice, AppValues,
    ErrorCode, PoolName, Request, Response, ShareName, VolumeNumber,
};
use zendiloos_apps::application::ports::{AppHost, AppHostError, HostValues};
use zendiloos_apps::domain::{AppVolume, Containers};
use zendiloos_kernel::{PortError, RequestContext};

use super::{WHAT, call_raw, unexpected};

/// Where the system agent keeps the apps' compose files: how their
/// Compose projects are told from others.
const APPS_CONFIGS: &str = "/etc/zendiloos/apps";

/// [`AppHost`] through the system agent (with a timeout long enough to
/// fetch images) and the files agent.
#[derive(Clone)]
pub struct AgentApps {
    system: Arc<dyn AgentTransport>,
    files: Arc<dyn AgentTransport>,
}

impl AgentApps {
    pub fn new(system: Arc<dyn AgentTransport>, files: Arc<dyn AgentTransport>) -> Self {
        Self { system, files }
    }
}

fn invalid(what: &str) -> AppHostError {
    AppHostError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn app_name(app: &str) -> Result<AppName, AppHostError> {
    AppName::parse(app).map_err(|_| invalid("app id"))
}

fn volume_number(number: u16) -> Result<VolumeNumber, AppHostError> {
    VolumeNumber::new(number).map_err(|_| invalid("volume number"))
}

/// The agent's answer, with its refusals and failures in its own words.
async fn ask(
    transport: &dyn AgentTransport,
    request: Request,
    context: &RequestContext,
) -> Result<Response, AppHostError> {
    match call_raw(transport, request, context).await? {
        Response::Error { code: ErrorCode::Forbidden | ErrorCode::Conflict, message } => {
            let reason = message.strip_prefix("refused: ").unwrap_or(&message);
            Err(AppHostError::Refused(reason.to_owned()))
        }
        Response::Error { message, .. } => Err(AppHostError::Failed(message)),
        response => Ok(response),
    }
}

async fn done(
    transport: &dyn AgentTransport,
    request: Request,
    context: &RequestContext,
) -> Result<(), AppHostError> {
    match ask(transport, request, context).await? {
        Response::Done => Ok(()),
        other => Err(unexpected(other).into()),
    }
}

/// `docker compose ls --all --format json`: the projects whose file is one
/// the agent keeps, with their containers counted.
fn statuses(compose_ls: &str) -> Result<BTreeMap<String, Containers>, PortError> {
    let projects: Value = serde_json::from_str(compose_ls)
        .map_err(|e| PortError::new(WHAT, format!("unexpected answer from Docker Compose: {e}")))?;
    let mut statuses = BTreeMap::new();
    for project in projects.as_array().into_iter().flatten() {
        let (Some(name), Some(status), Some(files)) = (
            project.get("Name").and_then(Value::as_str),
            project.get("Status").and_then(Value::as_str),
            project.get("ConfigFiles").and_then(Value::as_str),
        ) else {
            continue;
        };
        if files != format!("{APPS_CONFIGS}/{name}.json") {
            continue;
        }
        let mut containers = Containers::default();
        for part in status.split(',') {
            let Some((state, count)) = part.trim().strip_suffix(')').and_then(|p| p.split_once('(')) else {
                continue;
            };
            let count: u32 = count.parse().unwrap_or(0);
            match state {
                "running" => containers.running += count,
                "restarting" => containers.restarting += count,
                _ => containers.other += count,
            }
        }
        statuses.insert(name.to_owned(), containers);
    }
    Ok(statuses)
}

/// `text` without terminal control sequences (colours, cleared lines),
/// which apps write for a terminal and a browser shows as noise.
fn plain(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        // CSI: parameters and intermediates up to a final byte from @ to ~.
        if chars.next_if_eq(&'[').is_some() {
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else if matches!(chars.next(), Some('(' | ')' | '*' | '+')) {
            // A character set: one more character names it.
            chars.next();
        }
    }
    plain
}

#[async_trait]
impl AppHost for AgentApps {
    async fn prepare(
        &self,
        app: &str,
        volume: &AppVolume,
        context: &RequestContext,
    ) -> Result<(), AppHostError> {
        let request = Request::PrepareApp {
            app: app_name(app)?,
            pool: PoolName::parse(&volume.pool).map_err(|_| invalid("pool name"))?,
            volume: volume_number(volume.number)?,
        };
        done(self.system.as_ref(), request, context).await
    }

    async fn configure(
        &self,
        app: &str,
        volume: u16,
        manifest: &str,
        compose: &str,
        values: &HostValues,
        context: &RequestContext,
    ) -> Result<Vec<String>, AppHostError> {
        let mut agent_values = AppValues { ports: values.ports.clone(), ..AppValues::default() };
        for (id, share) in &values.shares {
            let choice = AppShareChoice {
                volume: volume_number(share.volume)?,
                name: ShareName::parse(&share.name).map_err(|_| invalid("share name"))?,
                folder: AppFolder::parse(&share.folder).map_err(|_| invalid("folder in a share"))?,
            };
            agent_values.shares.insert(id.clone(), choice);
        }
        for (id, value) in &values.settings {
            let value = AppSettingValue::new(value.clone()).map_err(|_| invalid("setting value"))?;
            agent_values.settings.insert(id.clone(), value);
        }
        let request = Request::ConfigureApp {
            app: app_name(app)?,
            volume: volume_number(volume)?,
            manifest: AppManifest::new(manifest).map_err(|_| invalid("manifest"))?,
            compose: AppCompose::new(compose).map_err(|_| invalid("compose file"))?,
            values: agent_values,
        };
        match ask(self.system.as_ref(), request, context).await? {
            Response::AppFolders { folders } => Ok(folders.into_iter().map(String::from).collect()),
            other => Err(unexpected(other).into()),
        }
    }

    async fn make_folders(
        &self,
        app: &str,
        volume: u16,
        folders: &[String],
        context: &RequestContext,
    ) -> Result<(), AppHostError> {
        let folders = folders
            .iter()
            .map(|folder| AppFolder::parse(folder).map_err(|_| invalid("folder")))
            .collect::<Result<Vec<_>, _>>()?;
        let request =
            Request::MakeAppFolders { app: app_name(app)?, volume: volume_number(volume)?, folders };
        done(self.files.as_ref(), request, context).await
    }

    async fn pull(&self, app: &str, context: &RequestContext) -> Result<(), AppHostError> {
        done(self.system.as_ref(), Request::PullApp { app: app_name(app)? }, context).await
    }

    async fn start(&self, app: &str, context: &RequestContext) -> Result<(), AppHostError> {
        done(self.system.as_ref(), Request::StartApp { app: app_name(app)? }, context).await
    }

    async fn stop(&self, app: &str, context: &RequestContext) -> Result<(), AppHostError> {
        done(self.system.as_ref(), Request::StopApp { app: app_name(app)? }, context).await
    }

    async fn remove(
        &self,
        app: &str,
        volume: &AppVolume,
        context: &RequestContext,
    ) -> Result<(), AppHostError> {
        let request = Request::RemoveApp {
            app: app_name(app)?,
            pool: PoolName::parse(&volume.pool).map_err(|_| invalid("pool name"))?,
            volume: volume_number(volume.number)?,
        };
        done(self.system.as_ref(), request, context).await
    }

    async fn delete_account(&self, app: &str, context: &RequestContext) -> Result<(), AppHostError> {
        done(self.system.as_ref(), Request::DeleteAppAccount { app: app_name(app)? }, context).await
    }

    async fn status(&self, context: &RequestContext) -> Result<BTreeMap<String, Containers>, AppHostError> {
        match ask(self.system.as_ref(), Request::AppsStatus {}, context).await? {
            Response::Apps { compose_ls } => Ok(statuses(&compose_ls)?),
            other => Err(unexpected(other).into()),
        }
    }

    async fn logs(&self, app: &str, lines: u32, context: &RequestContext) -> Result<String, AppHostError> {
        match ask(self.system.as_ref(), Request::AppLogs { app: app_name(app)?, lines }, context).await? {
            Response::AppLogs { text } => Ok(plain(&text)),
            other => Err(unexpected(other).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_apps::application::ports::HostShare;
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[test]
    fn only_the_projects_of_the_apps_count() {
        let compose_ls = r#"[
            {"Name": "immich", "Status": "running(3), restarting(1), exited(1)", "ConfigFiles": "/etc/zendiloos/apps/immich.json"},
            {"Name": "mine", "Status": "running(1)", "ConfigFiles": "/home/me/compose.yaml"},
            {"Name": "odd", "Status": "paused(2)", "ConfigFiles": "/etc/zendiloos/apps/odd.json"}
        ]"#;
        let apps = statuses(compose_ls).unwrap();
        assert_eq!(apps.len(), 2);
        assert_eq!(apps["immich"], Containers { running: 3, restarting: 1, other: 1 });
        assert_eq!(apps["odd"], Containers { running: 0, restarting: 0, other: 2 });
        assert!(statuses("not json").is_err());
    }

    #[test]
    fn logs_lose_terminal_colours() {
        let line = "\u{1b}[2Kuptime-kuma-1  | 2026-11-06T07:33:25Z \u{1b}[36mINFO:\u{1b}[0m Listening\n";
        assert_eq!(plain(line), "uptime-kuma-1  | 2026-11-06T07:33:25Z INFO: Listening\n");
        assert_eq!(plain("ä ö \u{1b}(Bok \u{1b}=x"), "ä ö ok x");
    }

    #[tokio::test]
    async fn an_app_runs_through_the_agents() {
        let agent = Arc::new(SimulatedAgent::new());
        let apps = AgentApps::new(agent.clone(), agent);
        let context = RequestContext::default();
        let volume = AppVolume { pool: "tank".into(), number: 1 };
        apps.prepare("demo", &volume, &context).await.unwrap();
        let values = HostValues {
            shares: BTreeMap::from([(
                "media".into(),
                HostShare { volume: 1, name: "family".into(), folder: "Films".into() },
            )]),
            ports: BTreeMap::from([("web".into(), 8080)]),
            settings: BTreeMap::from([("PASSWORD".into(), "s3cret".into())]),
        };
        let other = apps.configure("demo", 1, r#"{"id":"other"}"#, "services: {}", &values, &context).await;
        assert!(
            matches!(other, Err(AppHostError::Refused(ref reason)) if reason == "the manifest is another app's")
        );
        let folders = apps.configure("demo", 1, r#"{"id":"demo"}"#, "services: {}", &values, &context).await;
        assert_eq!(folders.unwrap(), Vec::<String>::new());
        apps.pull("demo", &context).await.unwrap();
        apps.start("demo", &context).await.unwrap();
        assert_eq!(apps.status(&context).await.unwrap()["demo"].running, 1);
        assert!(apps.logs("demo", 10, &context).await.unwrap().contains("started"));
        apps.remove("demo", &volume, &context).await.unwrap();
        assert!(apps.status(&context).await.unwrap().is_empty());
        assert!(matches!(apps.prepare("Demo!", &volume, &context).await, Err(AppHostError::Port(_))));
    }
}
