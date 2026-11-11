//! `/api/v1/ws`: one WebSocket per browser tab, with topic subscriptions.
//!
//! ```text
//! → {"type":"subscribe","topic":"metrics"}
//! ← {"type":"metrics","data":{…}}            the latest sample at once, then every new one
//! → {"type":"subscribe","topic":"vm_stats"}  (`vms.manage` only)
//! ← {"type":"vm_stats","data":{…}}           every few seconds, while anyone watches
//! ```
//!
//! Not part of the OpenAPI paths (it is no REST operation), but its message
//! types are, so the frontend's types are generated like the others.

use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{
    CloseFrame, Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection,
};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use futures_util::SinkExt;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use utoipa::ToSchema;
use zendiloos_kernel::Secret;
use zendiloos_system::application::ports::MetricsSubscription;
use zendiloos_system::domain::MetricsSample;

use zendiloos_identity::application::{AuthContext, IdentityError};
use zendiloos_identity::domain::Permission;
use zendiloos_vms::application::VmStatsSubscription;
use zendiloos_vms::domain::VmStatsSample;

use crate::auth::{Auth, check_same_origin, session_token};
use crate::error::ApiError;
use crate::routes::jobs::{sees_job, sees_notifications};
use crate::state::ApiState;

/// How often an open connection checks that its session is still valid.
const SESSION_CHECK: Duration = Duration::from_secs(60);
/// Close code sent when the session has ended (4000-4999: application codes).
const CLOSE_SESSION_ENDED: u16 = 4401;
/// Close code sent when the session could not be checked (the database did
/// not answer): the client connects again rather than signing out.
const CLOSE_TRY_AGAIN: u16 = 1011;
/// Client messages are tiny; refuse anything larger.
const MAX_CLIENT_MESSAGE: usize = 4 * 1024;

pub(crate) fn router() -> Router<ApiState> {
    Router::new().route(&format!("{}/ws", crate::API_V1), get(upgrade))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LiveTopic {
    Metrics,
    /// Every change to a job.
    Jobs,
    /// New notifications.
    Notifications,
    /// What the virtual machines use, for those who manage them.
    VmStats,
}

/// What the browser sends.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveRequest {
    Subscribe { topic: LiveTopic },
    Unsubscribe { topic: LiveTopic },
}

/// What the server sends.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveMessage {
    Metrics {
        data: Metrics,
    },
    Job {
        data: super::jobs::Job,
    },
    Notification {
        data: super::jobs::Notification,
    },
    VmStats {
        data: VmStats,
    },
    /// The last request could not be understood.
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Metrics {
    /// Unix time in seconds.
    pub at: i64,
    /// Busy share of all CPUs, 0 to 100.
    pub cpu_percent: f32,
    /// 1, 5 and 15 minute load averages.
    pub load_average: Vec<f64>,
    pub memory: MemoryMetrics,
    pub network: Vec<InterfaceMetrics>,
    pub disks: Vec<DiskMetrics>,
    pub temperatures: Vec<TemperatureMetrics>,
    pub uptime_seconds: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InterfaceMetrics {
    pub name: String,
    pub received_bytes_per_second: u64,
    pub sent_bytes_per_second: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DiskMetrics {
    pub name: String,
    pub read_bytes_per_second: u64,
    pub written_bytes_per_second: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TemperatureMetrics {
    pub label: String,
    pub celsius: f32,
}

/// What the virtual machines use now.
#[derive(Debug, Serialize, ToSchema)]
pub struct VmStats {
    /// Unix time in seconds.
    pub at: i64,
    /// Each running VM's load.
    pub vms: Vec<VmLoad>,
    pub running: usize,
    pub total: usize,
    /// The share of the machine's threads the VMs keep busy, 0 to 100.
    pub cpu_percent: f32,
    /// What the running VMs were given, against the machine's memory.
    pub memory_assigned_bytes: u64,
    pub host_memory_bytes: u64,
    pub host_cores: u32,
}

/// A running VM's load. What libvirt cannot tell is none: the CPU on the
/// first look, the network for a card through passt (it has no device on
/// the host to count).
#[derive(Debug, Serialize, ToSchema)]
pub struct VmLoad {
    pub id: String,
    /// Busy share of its vCPUs, 0 to 100.
    pub cpu_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    /// What it was given.
    pub memory_bytes: u64,
    pub received_bytes_per_second: Option<u64>,
    pub sent_bytes_per_second: Option<u64>,
}

impl From<VmStatsSample> for VmStats {
    fn from(sample: VmStatsSample) -> Self {
        Self {
            at: sample.at.unix_seconds(),
            vms: sample
                .vms
                .into_iter()
                .map(|load| VmLoad {
                    id: load.id,
                    cpu_percent: load.cpu_percent,
                    memory_used_bytes: load.memory_used_bytes,
                    memory_bytes: load.memory_bytes,
                    received_bytes_per_second: load.received_bytes_per_second,
                    sent_bytes_per_second: load.sent_bytes_per_second,
                })
                .collect(),
            running: sample.running,
            total: sample.total,
            cpu_percent: sample.cpu_percent,
            memory_assigned_bytes: sample.memory_assigned_bytes,
            host_memory_bytes: sample.host_memory_bytes,
            host_cores: sample.host_cores,
        }
    }
}

impl From<MetricsSample> for Metrics {
    fn from(sample: MetricsSample) -> Self {
        Self {
            at: sample.at.unix_seconds(),
            cpu_percent: sample.cpu_percent,
            load_average: sample.load_average.to_vec(),
            memory: MemoryMetrics {
                total_bytes: sample.memory.total_bytes,
                used_bytes: sample.memory.used_bytes,
                available_bytes: sample.memory.available_bytes,
            },
            network: sample
                .network
                .into_iter()
                .map(|n| InterfaceMetrics {
                    name: n.name,
                    received_bytes_per_second: n.received_bytes_per_second,
                    sent_bytes_per_second: n.sent_bytes_per_second,
                })
                .collect(),
            disks: sample
                .disks
                .into_iter()
                .map(|d| DiskMetrics {
                    name: d.name,
                    read_bytes_per_second: d.read_bytes_per_second,
                    written_bytes_per_second: d.written_bytes_per_second,
                })
                .collect(),
            temperatures: sample
                .temperatures
                .into_iter()
                .map(|t| TemperatureMetrics { label: t.label, celsius: t.celsius })
                .collect(),
            uptime_seconds: sample.uptime_seconds,
        }
    }
}

async fn upgrade(
    auth: Auth,
    State(state): State<ApiState>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    // The upgrade is a GET, which carries no CSRF token: without this check
    // any page could open a socket with the visitor's cookie.
    check_same_origin(&headers)?;
    let upgrade = upgrade.map_err(|rejection| ApiError::BadRequest(rejection.body_text()))?;
    let token = session_token(&headers, state.secure_cookies).ok_or(ApiError::Unauthenticated)?;
    Ok(upgrade
        .max_message_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| serve(socket, state, token, auth.0)))
}

enum Event {
    Incoming(Option<Result<Message, axum::Error>>),
    Sample(Option<MetricsSample>),
    Job(Result<zendiloos_jobs::Job, broadcast::error::RecvError>),
    Notification(Result<zendiloos_notify::Notification, broadcast::error::RecvError>),
    VmStats(Option<VmStatsSample>),
    CheckSession,
}

async fn next_vm_stats(subscription: &mut Option<VmStatsSubscription>) -> Option<VmStatsSample> {
    match subscription {
        Some(subscription) => subscription.next().await,
        None => std::future::pending().await,
    }
}

async fn next_notification(
    notifications: &mut Option<broadcast::Receiver<zendiloos_notify::Notification>>,
) -> Result<zendiloos_notify::Notification, broadcast::error::RecvError> {
    match notifications {
        Some(notifications) => notifications.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_job(
    jobs: &mut Option<broadcast::Receiver<zendiloos_jobs::Job>>,
) -> Result<zendiloos_jobs::Job, broadcast::error::RecvError> {
    match jobs {
        Some(jobs) => jobs.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_sample(subscription: &mut Option<Box<dyn MetricsSubscription>>) -> Option<MetricsSample> {
    match subscription {
        Some(subscription) => subscription.next().await,
        None => std::future::pending().await,
    }
}

/// How a connection ends when its session check fails: only a session that
/// is really gone signs the browser out (4401).
fn closing(error: &IdentityError) -> CloseFrame {
    match error {
        IdentityError::Unauthenticated => {
            CloseFrame { code: CLOSE_SESSION_ENDED, reason: "session ended".into() }
        }
        _ => CloseFrame { code: CLOSE_TRY_AGAIN, reason: "session not checked".into() },
    }
}

async fn send(socket: &mut WebSocket, message: &LiveMessage) -> bool {
    let text = serde_json::to_string(message).expect("live messages serialize");
    socket.send(Message::Text(text.into())).await.is_ok()
}

async fn serve(mut socket: WebSocket, state: ApiState, token: Secret, mut viewer: AuthContext) {
    let mut metrics: Option<Box<dyn MetricsSubscription>> = None;
    let mut jobs: Option<broadcast::Receiver<zendiloos_jobs::Job>> = None;
    let mut notifications: Option<broadcast::Receiver<zendiloos_notify::Notification>> = None;
    let mut vm_stats: Option<VmStatsSubscription> = None;
    let mut session_check = tokio::time::interval(SESSION_CHECK);
    session_check.tick().await;

    loop {
        let event = tokio::select! {
            incoming = socket.recv() => Event::Incoming(incoming),
            sample = next_sample(&mut metrics) => Event::Sample(sample),
            job = next_job(&mut jobs) => Event::Job(job),
            notification = next_notification(&mut notifications) => Event::Notification(notification),
            sample = next_vm_stats(&mut vm_stats) => Event::VmStats(sample),
            _ = session_check.tick() => Event::CheckSession,
        };
        let delivered = match event {
            Event::Incoming(Some(Ok(Message::Text(text)))) => match serde_json::from_str(&text) {
                Ok(LiveRequest::Subscribe { topic: LiveTopic::Metrics }) => {
                    metrics = Some(state.system.watch_metrics.subscribe());
                    match state.system.watch_metrics.latest() {
                        Some(sample) => {
                            send(&mut socket, &LiveMessage::Metrics { data: sample.into() }).await
                        }
                        None => true,
                    }
                }
                Ok(LiveRequest::Unsubscribe { topic: LiveTopic::Metrics }) => {
                    metrics = None;
                    true
                }
                Ok(LiveRequest::Subscribe { topic: LiveTopic::Jobs }) => {
                    jobs = Some(state.jobs.subscribe());
                    true
                }
                Ok(LiveRequest::Unsubscribe { topic: LiveTopic::Jobs }) => {
                    jobs = None;
                    true
                }
                Ok(LiveRequest::Subscribe { topic: LiveTopic::Notifications }) => {
                    // Accepted either way; only those who may see them get any.
                    if sees_notifications(&viewer) {
                        notifications = Some(state.notifier.subscribe());
                    }
                    true
                }
                Ok(LiveRequest::Unsubscribe { topic: LiveTopic::Notifications }) => {
                    notifications = None;
                    true
                }
                Ok(LiveRequest::Subscribe { topic: LiveTopic::VmStats }) => {
                    // Accepted either way; only those who manage VMs get any.
                    if viewer.can(Permission::VmsManage) {
                        let subscription = state.vms.watch_stats();
                        let latest = subscription.latest();
                        vm_stats = Some(subscription);
                        match latest {
                            Some(sample) => {
                                send(&mut socket, &LiveMessage::VmStats { data: sample.into() }).await
                            }
                            None => true,
                        }
                    } else {
                        true
                    }
                }
                Ok(LiveRequest::Unsubscribe { topic: LiveTopic::VmStats }) => {
                    vm_stats = None;
                    true
                }
                Err(error) => send(&mut socket, &LiveMessage::Error { message: error.to_string() }).await,
            },
            Event::Incoming(Some(Ok(Message::Close(_)))) => {
                // Completes the closing handshake: sends the queued reply.
                let _ = socket.close().await;
                break;
            }
            Event::Incoming(None | Some(Err(_))) => break,
            Event::Incoming(Some(Ok(_))) => true,
            Event::Sample(Some(sample)) => {
                send(&mut socket, &LiveMessage::Metrics { data: sample.into() }).await
            }
            Event::Sample(None) => {
                metrics = None;
                true
            }
            Event::Job(Ok(job)) if sees_job(&viewer, &job) => {
                send(&mut socket, &LiveMessage::Job { data: job.into() }).await
            }
            Event::Job(Ok(_)) => true,
            // Missed changes: the client reloads the list when it sees a gap.
            Event::Job(Err(broadcast::error::RecvError::Lagged(_))) => true,
            Event::Job(Err(broadcast::error::RecvError::Closed)) => {
                jobs = None;
                true
            }
            Event::Notification(Ok(notification)) => {
                send(&mut socket, &LiveMessage::Notification { data: notification.into() }).await
            }
            Event::Notification(Err(broadcast::error::RecvError::Lagged(_))) => true,
            Event::Notification(Err(broadcast::error::RecvError::Closed)) => {
                notifications = None;
                true
            }
            Event::VmStats(Some(sample)) => {
                send(&mut socket, &LiveMessage::VmStats { data: sample.into() }).await
            }
            Event::VmStats(None) => {
                vm_stats = None;
                true
            }
            Event::CheckSession => match state.identity.authenticate.verify(&token).await {
                // What the account may see now: a role taken away also ends
                // what it was subscribed to.
                Ok(fresh) => {
                    viewer = fresh;
                    if !sees_notifications(&viewer) {
                        notifications = None;
                    }
                    if !viewer.can(Permission::VmsManage) {
                        vm_stats = None;
                    }
                    true
                }
                Err(error) => {
                    let _ = socket.send(Message::Close(Some(closing(&error)))).await;
                    break;
                }
            },
        };
        if !delivered {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::PortError;

    use super::*;

    #[test]
    fn only_a_session_that_is_gone_signs_out() {
        assert_eq!(closing(&IdentityError::Unauthenticated).code, CLOSE_SESSION_ENDED);
        let unanswered = IdentityError::Port(PortError::new("sessions", "database is locked"));
        assert_eq!(closing(&unanswered).code, CLOSE_TRY_AGAIN, "the browser connects again");
    }
}
