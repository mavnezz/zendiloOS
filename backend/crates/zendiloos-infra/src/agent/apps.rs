//! Apps on the host, through the system agent: what filling in a template
//! takes from the machine, and running and removing the apps installed the
//! earlier way.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use zendiloos_agent_proto::{
    AgentTransport, AppName, DevicePath, ErrorCode, PoolName, Request, Response, UserName, VolumeNumber,
};
use zendiloos_apps::application::ports::{AppHost, AppHostError, TemplateHost};
use zendiloos_apps::domain::{AppVolume, Containers, HostFacts};
use zendiloos_kernel::{PortError, RequestContext};

use super::{WHAT, call_raw, unexpected};

/// Where the system agent keeps the apps' compose files: how their
/// Compose projects are told from others.
const APPS_CONFIGS: &str = "/etc/zendiloos/apps";

/// [`AppHost`] and [`TemplateHost`] through the system agent (with a
/// timeout long enough to stop and remove containers).
#[derive(Clone)]
pub struct AgentApps {
    system: Arc<dyn AgentTransport>,
}

impl AgentApps {
    pub fn new(system: Arc<dyn AgentTransport>) -> Self {
        Self { system }
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
impl TemplateHost for AgentApps {
    async fn facts(
        &self,
        user: &str,
        devices: &[String],
        context: &RequestContext,
    ) -> Result<HostFacts, AppHostError> {
        let user = UserName::parse(user).map_err(|_| invalid("user name"))?;
        let devices = devices
            .iter()
            .map(|device| DevicePath::parse(device).map_err(|_| invalid("device")))
            .collect::<Result<Vec<_>, _>>()?;
        match ask(self.system.as_ref(), Request::HostFacts { user, devices }, context).await? {
            Response::HostFacts { uid, gid, time_zone, devices } => Ok(HostFacts {
                uid,
                gid,
                time_zone,
                devices: devices.into_iter().map(String::from).collect(),
            }),
            other => Err(unexpected(other).into()),
        }
    }
}

#[async_trait]
impl AppHost for AgentApps {
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
    async fn an_app_installed_before_runs_through_the_agent() {
        let apps = AgentApps::new(Arc::new(SimulatedAgent::new().with_app("demo")));
        let context = RequestContext::default();
        let volume = AppVolume { pool: "tank".into(), number: 1 };
        apps.start("demo", &context).await.unwrap();
        assert_eq!(apps.status(&context).await.unwrap()["demo"].running, 1);
        assert!(apps.logs("demo", 10, &context).await.unwrap().contains("started"));
        apps.stop("demo", &context).await.unwrap();
        apps.remove("demo", &volume, &context).await.unwrap();
        apps.delete_account("demo", &context).await.unwrap();
        assert!(apps.status(&context).await.unwrap().is_empty());
        let unknown = apps.start("other", &context).await;
        assert!(
            matches!(unknown, Err(AppHostError::Refused(ref reason)) if reason == "other is not installed")
        );
        assert!(matches!(apps.start("Demo!", &context).await, Err(AppHostError::Port(_))));
    }

    #[tokio::test]
    async fn the_machine_says_what_a_template_needs() {
        let apps = AgentApps::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let devices = ["/dev/dri".to_owned(), "/dev/ttyUSB0".to_owned()];

        let facts = apps.facts("alice", &devices, &context).await.unwrap();

        assert_eq!(
            facts,
            HostFacts {
                uid: 2000,
                gid: 1500,
                time_zone: "Europe/Berlin".into(),
                devices: vec!["/dev/dri".into()]
            }
        );
        assert!(matches!(apps.facts("Alice Smith", &[], &context).await, Err(AppHostError::Port(_))));
        let odd = ["/etc/passwd".to_owned()];
        assert!(matches!(apps.facts("alice", &odd, &context).await, Err(AppHostError::Port(_))));
    }
}
