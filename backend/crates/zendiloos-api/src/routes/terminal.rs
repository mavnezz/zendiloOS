//! `/api/v1/docker/containers/{id}/terminal`: a terminal in a running
//! container over a WebSocket (`containers.manage`).
//!
//! ```text
//! GET …/terminal?command=/bin/bash&columns=120&rows=40   (upgraded)
//! → binary frames: what is typed
//! → {"type":"resize","columns":120,"rows":40}
//! ← binary frames: what the program wrote
//! ← {"type":"ended","reason":"exited"}   then the socket closes
//! ← {"type":"error","message":"…"}       it could not be opened
//! ```
//!
//! Not part of the OpenAPI paths (it is no REST operation), but its message
//! types are, so the frontend's types are generated like the others.

use std::time::Duration;

use axum::Router;
use axum::extract::ws::{
    CloseFrame, Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection,
};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use futures_util::SinkExt;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use zendiloos_containers::application::ContainersError;
use zendiloos_containers::application::ports::TerminalInput;
use zendiloos_identity::application::AuthContext;
use zendiloos_identity::domain::Permission;
use zendiloos_kernel::{RequestContext, Secret};

use crate::auth::{Auth, check_same_origin, session_token};
use crate::error::ApiError;
use crate::extract::{ApiPath as Path, Client};
use crate::state::ApiState;

/// How often the session behind an open terminal is checked again.
const SESSION_CHECK: Duration = Duration::from_secs(60);
/// A ping keeps the reverse proxy from closing a terminal nothing is written
/// to.
const PING_EVERY: Duration = Duration::from_secs(30);
/// Nothing typed for this long ends the terminal.
const IDLE_LIMIT: Duration = Duration::from_secs(30 * 60);
/// What is typed or pasted at once.
const MAX_CLIENT_MESSAGE: usize = 64 * 1024;
/// The program run unless another is named.
const DEFAULT_COMMAND: &str = "/bin/sh";

pub(crate) fn router() -> Router<ApiState> {
    Router::new().route(&format!("{}/docker/containers/{{id}}/terminal", crate::API_V1), get(upgrade))
}

#[derive(Debug, Deserialize)]
pub(crate) struct TerminalParams {
    /// The program and its arguments, as typed; `/bin/sh` if absent.
    command: Option<String>,
    columns: Option<u16>,
    rows: Option<u16>,
}

/// What the browser sends as text; what is typed comes as binary frames.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminalRequest {
    Resize { columns: u16, rows: u16 },
}

/// Why a terminal ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalEnd {
    /// The program ended.
    Exited,
    /// Nothing was typed for half an hour.
    Idle,
    /// The session ended, or the account may no longer manage containers.
    Session,
}

/// What the server sends as text; what the program writes comes as binary
/// frames.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalMessage {
    Ended {
        reason: TerminalEnd,
    },
    /// The terminal could not be opened, or a message was not understood.
    Error {
        message: String,
    },
}

async fn upgrade(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<TerminalParams>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    // The upgrade is a GET, which carries no CSRF token: without this check
    // any page could open a terminal with the visitor's cookie.
    check_same_origin(&headers)?;
    auth.require(Permission::ContainersManage)?;
    let upgrade = upgrade.map_err(|rejection| ApiError::BadRequest(rejection.body_text()))?;
    let token = session_token(&headers, state.secure_cookies).ok_or(ApiError::Unauthenticated)?;
    Ok(upgrade
        .max_message_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| serve(socket, state, token, auth.0, client, id, params)))
}

async fn say(socket: &mut WebSocket, message: &TerminalMessage) -> bool {
    let text = serde_json::to_string(message).expect("terminal messages serialize");
    socket.send(Message::Text(text.into())).await.is_ok()
}

/// Says why it ended and closes the socket.
async fn end(socket: &mut WebSocket, reason: TerminalEnd) {
    let _ = say(socket, &TerminalMessage::Ended { reason }).await;
    let _ = socket.send(Message::Close(Some(CloseFrame { code: 1000, reason: "ended".into() }))).await;
}

enum Event {
    Output(Option<Vec<u8>>),
    Incoming(Option<Result<Message, axum::Error>>),
    Ping,
    CheckSession,
    Idle,
}

async fn serve(
    mut socket: WebSocket,
    state: ApiState,
    token: Secret,
    viewer: AuthContext,
    client: RequestContext,
    id: String,
    params: TerminalParams,
) {
    let command = params.command.unwrap_or_else(|| DEFAULT_COMMAND.to_owned());
    let (columns, rows) = (params.columns.unwrap_or(80), params.rows.unwrap_or(24));
    let mut terminal =
        match state.containers.open_terminal(&id, &command, columns, rows, &viewer.actor(), &client).await {
            Ok(terminal) => terminal,
            Err(error) => {
                // What went wrong inside stays in the log.
                let message = match error {
                    ContainersError::Port(error) => {
                        tracing::error!(%error, "cannot open a terminal");
                        "the terminal could not be opened".to_owned()
                    }
                    other => other.to_string(),
                };
                let _ = say(&mut socket, &TerminalMessage::Error { message }).await;
                let _ = socket.close().await;
                return;
            }
        };
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut session_check = tokio::time::interval(SESSION_CHECK);
    session_check.tick().await;
    let mut typed_at = tokio::time::Instant::now();

    loop {
        let event = tokio::select! {
            output = terminal.session.output.recv() => Event::Output(output),
            incoming = socket.recv() => Event::Incoming(incoming),
            _ = ping.tick() => Event::Ping,
            _ = session_check.tick() => Event::CheckSession,
            () = tokio::time::sleep_until(typed_at + IDLE_LIMIT) => Event::Idle,
        };
        match event {
            Event::Output(Some(data)) => {
                if socket.send(Message::Binary(data.into())).await.is_err() {
                    break;
                }
            }
            Event::Output(None) => {
                end(&mut socket, TerminalEnd::Exited).await;
                break;
            }
            Event::Incoming(Some(Ok(Message::Binary(data)))) => {
                typed_at = tokio::time::Instant::now();
                if terminal.session.input.send(TerminalInput::Data(data.to_vec())).await.is_err() {
                    end(&mut socket, TerminalEnd::Exited).await;
                    break;
                }
            }
            Event::Incoming(Some(Ok(Message::Text(text)))) => match serde_json::from_str(&text) {
                Ok(TerminalRequest::Resize { columns, rows }) => {
                    let _ = terminal.session.input.send(TerminalInput::Resize { columns, rows }).await;
                }
                Err(error) => {
                    if !say(&mut socket, &TerminalMessage::Error { message: error.to_string() }).await {
                        break;
                    }
                }
            },
            Event::Incoming(Some(Ok(Message::Close(_)))) => {
                let _ = socket.close().await;
                break;
            }
            Event::Incoming(None | Some(Err(_))) => break,
            Event::Incoming(Some(Ok(_))) => {}
            Event::Ping => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            Event::CheckSession => match state.identity.authenticate.verify(&token).await {
                Ok(fresh) if fresh.can(Permission::ContainersManage) => {}
                _ => {
                    end(&mut socket, TerminalEnd::Session).await;
                    break;
                }
            },
            Event::Idle => {
                end(&mut socket, TerminalEnd::Idle).await;
                break;
            }
        }
    }
    // Dropping the terminal hangs it up, which ends its program.
    drop(terminal);
}
