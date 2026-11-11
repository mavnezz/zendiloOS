//! `/api/v1/vms/{id}/console`: a running VM's console over a WebSocket
//! (`vms.manage`), for noVNC in the browser.
//!
//! ```text
//! GET …/console   (upgraded, subprotocol `binary` if asked for)
//! ↔ binary frames: RFB, as QEMU's VNC server and the browser speak it
//! ← close 1000 "ended"        the console closed (the VM stopped)
//! ← close 4001 "session"      the session ended or lost the permission
//! ← close 4000 <reason>       it could not be opened
//! ```
//!
//! zendiloosd only relays bytes: the agent connected to QEMU's socket and
//! handed it over, as it hands over a terminal. The socket never carries
//! text of zendiloOS's own, so RFB is all noVNC reads; why it ended is said
//! in the close frame.

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
use zendiloos_identity::application::AuthContext;
use zendiloos_identity::domain::Permission;
use zendiloos_kernel::{RequestContext, Secret};
use zendiloos_vms::application::VmsError;

use crate::auth::{Auth, check_same_origin, session_token};
use crate::error::ApiError;
use crate::extract::{ApiPath as Path, Client};
use crate::state::ApiState;

/// How often the session behind an open console is checked again.
const SESSION_CHECK: Duration = Duration::from_secs(60);
/// A ping keeps the reverse proxy from closing a console nothing happens on.
const PING_EVERY: Duration = Duration::from_secs(30);
/// What the browser sends at once: keys, the pointer, a pasted text.
const MAX_CLIENT_MESSAGE: usize = 256 * 1024;

pub(crate) fn router() -> Router<ApiState> {
    Router::new().route(&format!("{}/vms/{{id}}/console", crate::API_V1), get(upgrade))
}

async fn upgrade(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    headers: HeaderMap,
    Path(id): Path<String>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    // The upgrade is a GET, which carries no CSRF token: without this check
    // any page could open a console with the visitor's cookie.
    check_same_origin(&headers)?;
    auth.require(Permission::VmsManage)?;
    let upgrade = upgrade.map_err(|rejection| ApiError::BadRequest(rejection.body_text()))?;
    let token = session_token(&headers, state.secure_cookies).ok_or(ApiError::Unauthenticated)?;
    Ok(upgrade
        .protocols(["binary"])
        .max_message_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| serve(socket, state, token, auth.0, client, id)))
}

async fn close(socket: &mut WebSocket, code: u16, reason: &str) {
    // A close frame's reason has 123 bytes at most.
    let mut end = reason.len().min(120);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let frame = CloseFrame { code, reason: reason[..end].to_owned().into() };
    let _ = socket.send(Message::Close(Some(frame))).await;
}

enum Event {
    Output(Option<Vec<u8>>),
    Incoming(Option<Result<Message, axum::Error>>),
    Ping,
    CheckSession,
}

async fn serve(
    mut socket: WebSocket,
    state: ApiState,
    token: Secret,
    viewer: AuthContext,
    client: RequestContext,
    id: String,
) {
    let console = match state.vms.open_console(&id, &viewer.actor(), &client).await {
        Ok(console) => console,
        Err(error) => {
            // What went wrong inside stays in the log.
            let reason = match error {
                VmsError::Port(error) => {
                    tracing::error!(%error, "cannot open a virtual machine's console");
                    "the console could not be opened".to_owned()
                }
                other => other.to_string(),
            };
            close(&mut socket, 4000, &reason).await;
            return;
        }
    };
    let mut session = console.session;
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut session_check = tokio::time::interval(SESSION_CHECK);
    session_check.tick().await;

    loop {
        let event = tokio::select! {
            output = session.output.recv() => Event::Output(output),
            incoming = socket.recv() => Event::Incoming(incoming),
            _ = ping.tick() => Event::Ping,
            _ = session_check.tick() => Event::CheckSession,
        };
        match event {
            Event::Output(Some(data)) => {
                if socket.send(Message::Binary(data.into())).await.is_err() {
                    break;
                }
            }
            Event::Output(None) => {
                close(&mut socket, 1000, "ended").await;
                break;
            }
            Event::Incoming(Some(Ok(Message::Binary(data)))) => {
                if session.input.send(data.to_vec()).await.is_err() {
                    close(&mut socket, 1000, "ended").await;
                    break;
                }
            }
            Event::Incoming(Some(Ok(Message::Close(_)))) => {
                let _ = socket.close().await;
                break;
            }
            Event::Incoming(None | Some(Err(_))) => break,
            // Text has no place in RFB; pongs need nothing.
            Event::Incoming(Some(Ok(_))) => {}
            Event::Ping => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            Event::CheckSession => match state.identity.authenticate.verify(&token).await {
                Ok(fresh) if fresh.can(Permission::VmsManage) => {}
                _ => {
                    close(&mut socket, 4001, "session").await;
                    break;
                }
            },
        }
    }
    // Dropping the console closes the connection to QEMU.
    drop(session);
}
