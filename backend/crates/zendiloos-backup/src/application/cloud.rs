//! Cloud accounts (Google Drive, OneDrive): signed in to from the browser
//! without rclone on the administrator's computer. The NAS shows the
//! provider's consent page with rclone's public client (or the
//! administrator's own); after consent the browser lands on
//! `http://127.0.0.1:53682/`, which it cannot reach, and the administrator
//! pastes that address back; zendiloosd exchanges its code for a token.

use std::time::{Duration, Instant};

use zendiloos_kernel::{Actor, RequestContext, Secret};

use super::ports::NewAccount;
use super::{BackupError, Backups, audited};
use crate::domain::{CloudAccount, Invalid, Place, Provider, Redirect, read_redirect};

/// How long a sign-in waits for its address to be pasted.
const FLOW_FOR: Duration = Duration::from_secs(30 * 60);
/// Sign-ins under way at once.
const FLOWS: usize = 20;

/// A sign-in under way.
pub(super) struct Flow {
    name: String,
    provider: Provider,
    client_id: Option<String>,
    client_secret: Option<Secret>,
    started: Instant,
}

/// A sign-in begun: the page to consent on, and its state, to finish with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    pub flow: String,
    pub url: String,
}

impl Backups {
    pub async fn accounts(&self) -> Result<Vec<CloudAccount>, BackupError> {
        Ok(self.ports.accounts.list().await?)
    }

    /// Begins signing in to `provider` as an account called `name`, with the
    /// administrator's own client if given.
    pub fn start_sign_in(
        &self,
        provider: Provider,
        name: &str,
        client_id: Option<String>,
        client_secret: Option<Secret>,
    ) -> Result<SignIn, BackupError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
            return Err(Invalid("an account's name has 1 to 64 characters".into()).into());
        }
        let client_id = client_id.map(|id| id.trim().to_owned()).filter(|id| !id.is_empty());
        if client_id.as_ref().is_some_and(|id| id.len() > 256 || !id.bytes().all(|b| b.is_ascii_graphic())) {
            return Err(Invalid("that is no client ID".into()).into());
        }
        let client_secret = client_secret.filter(|s| !s.expose().is_empty());
        let state = self.ports.sign_in.new_state();
        let url = self.ports.sign_in.consent_url(provider, client_id.as_deref(), &state);
        let mut flows = self.flows.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        flows.retain(|_, flow| flow.started.elapsed() < FLOW_FOR);
        while flows.len() >= FLOWS {
            let oldest = flows.iter().min_by_key(|(_, flow)| flow.started).map(|(state, _)| state.clone());
            if let Some(oldest) = oldest {
                flows.remove(&oldest);
            }
        }
        flows.insert(
            state.clone(),
            Flow { name: name.to_owned(), provider, client_id, client_secret, started: Instant::now() },
        );
        Ok(SignIn { flow: state, url })
    }

    /// Finishes a sign-in with the address the browser landed on: its code
    /// exchanged for a token, kept as a secret with a new account.
    pub async fn finish_sign_in(
        &self,
        flow: &str,
        pasted: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<CloudAccount, BackupError> {
        let code = match read_redirect(pasted) {
            Some(Redirect::Code { code, state }) if state == flow => code,
            Some(Redirect::Code { .. }) => {
                return Err(
                    Invalid("that address belongs to another sign-in: paste the newest one".into()).into()
                );
            }
            Some(Redirect::Error(reason)) => return Err(BackupError::Refused(reason)),
            None => {
                return Err(Invalid("that address carries no code: paste the whole address".into()).into());
            }
        };
        let pending = {
            let mut flows = self.flows.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            flows.remove(flow).filter(|pending| pending.started.elapsed() < FLOW_FOR)
        };
        let pending = pending.ok_or(BackupError::SignInExpired)?;
        let done = async {
            let sign_in = &self.ports.sign_in;
            let token = sign_in
                .exchange(
                    pending.provider,
                    pending.client_id.as_deref(),
                    pending.client_secret.as_ref(),
                    &code,
                )
                .await?;
            let drive = sign_in.drive(pending.provider, &token).await?;
            let new = NewAccount {
                name: pending.name.clone(),
                provider: pending.provider,
                client_id: pending.client_id.clone(),
                drive_id: drive.as_ref().map(|(id, _)| id.clone()),
                drive_type: drive.map(|(_, kind)| kind),
                created_at: self.now(),
            };
            let id = self
                .ports
                .accounts
                .create(&new, &token, pending.client_secret.as_ref())
                .await?
                .ok_or(BackupError::NameTaken)?;
            self.ports.accounts.find(id).await?.ok_or(BackupError::NotFound)
        }
        .await;
        self.audit("backup.cloud_sign_in", audited(&done), &pending.name, Some(actor), context).await;
        done
    }

    /// Forgets a cloud account and its token; not while tasks back up into it.
    pub async fn delete_account(
        &self,
        id: i64,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), BackupError> {
        let account = self.ports.accounts.find(id).await?.ok_or(BackupError::NotFound)?;
        let users: Vec<String> = self
            .ports
            .tasks
            .list()
            .await?
            .into_iter()
            .filter(|task| matches!(&task.settings.target, Place::Cloud(cloud) if cloud.account == id))
            .map(|task| task.settings.name)
            .collect();
        if !users.is_empty() {
            return Err(BackupError::AccountInUse(users));
        }
        let done = self.ports.accounts.delete(id).await;
        self.audit("backup.cloud_delete", audited(&done), &account.name, Some(actor), context).await;
        done?;
        Ok(())
    }
}
