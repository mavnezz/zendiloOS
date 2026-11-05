//! Commands of administrators on other accounts. Each one keeps the
//! database, the Linux user and its Samba entry in step: the host is
//! changed first, because it is where a change is most likely to fail, and
//! a failed creation is undone.

use std::sync::Arc;

use zendiloos_kernel::{AuditEntry, Outcome, RequestContext, Secret};

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{
    Account, AccountId, AccountName, AuditAction, NewAccount, NewPassword, Permission, Role, text,
};

pub struct NewAccountInput {
    pub name: String,
    pub full_name: Option<String>,
    pub role: Role,
    /// Initial password; the account must change it at the first login.
    pub password: Secret,
}

/// Command: an administrator creates an account.
#[derive(Clone)]
pub struct CreateAccount {
    context: Arc<Context>,
}

impl CreateAccount {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        input: NewAccountInput,
        request: &RequestContext,
    ) -> Result<Account, IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::UsersManage)?;
        let name = AccountName::parse(input.name.trim())?;
        let full_name = text::full_name(input.full_name.as_deref())?;
        let password = NewPassword::parse(input.password, &name)?;

        let _changes = c.changes.lock().await;
        let now = c.now();
        if c.ports.accounts.find_by_name(&name).await?.is_some() {
            return Err(IdentityError::NameTaken);
        }
        c.provision_user(&name, password.secret(), request).await?;
        let stored = async {
            let password_hash = c.ports.hasher.hash(password.secret()).await?;
            let new = NewAccount {
                name: name.clone(),
                full_name,
                role: input.role,
                password_hash,
                password_change_required: true,
                created_at: now,
            };
            c.ports.accounts.create(new).await?.ok_or(IdentityError::NameTaken)
        }
        .await;
        let account = match stored {
            Ok(account) => account,
            Err(error) => {
                c.unprovision_user(&name, request).await;
                return Err(error);
            }
        };

        let entry = AuditEntry::new(now, AuditAction::AccountCreate.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(name.as_str())).await?;
        Ok(account)
    }
}

/// Changes to an account; `None` leaves a field as it is.
#[derive(Default)]
pub struct AccountChanges {
    /// `Some(None)` removes the full name.
    pub full_name: Option<Option<String>>,
    pub role: Option<Role>,
    pub disabled: Option<bool>,
}

/// Command: an administrator changes another account's name, role or
/// whether it is enabled. Disabling ends its sessions and its SMB access.
#[derive(Clone)]
pub struct UpdateAccount {
    context: Arc<Context>,
}

impl UpdateAccount {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        id: AccountId,
        changes: AccountChanges,
        request: &RequestContext,
    ) -> Result<Account, IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::UsersManage)?;
        let full_name = changes.full_name.map(|name| text::full_name(name.as_deref())).transpose()?;

        let _changes = c.changes.lock().await;
        let now = c.now();
        let mut account = c.ports.accounts.find_by_id(id).await?.ok_or(IdentityError::AccountNotFound)?;
        let role = changes.role.unwrap_or(account.role);
        let disabled = changes.disabled.unwrap_or(account.disabled);
        let access_changes = role != account.role || disabled != account.disabled;
        if access_changes && id == auth.account.id {
            return Err(IdentityError::OwnAccount);
        }
        let loses_admin =
            account.role == Role::Admin && !account.disabled && (role != Role::Admin || disabled);
        if loses_admin && !c.another_enabled_admin(id).await? {
            return Err(IdentityError::LastAdmin);
        }

        if disabled != account.disabled {
            c.ports.system.set_enabled(&account.name, !disabled, request).await?;
            if disabled {
                c.ports.sessions.delete_all(id).await?;
            }
        }
        if let Some(full_name) = full_name {
            account.full_name = full_name;
        }
        account.role = role;
        account.disabled = disabled;
        account.updated_at = now;
        c.ports.accounts.update(&account).await?;

        let entry = AuditEntry::new(now, AuditAction::AccountUpdate.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(account.name.as_str())).await?;
        Ok(account)
    }
}

/// Command: an administrator sets a new password for another account,
/// e.g. after it was forgotten. The account must change it at the next
/// login, and its sessions end.
#[derive(Clone)]
pub struct ResetPassword {
    context: Arc<Context>,
}

impl ResetPassword {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        id: AccountId,
        password: Secret,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::UsersManage)?;
        if id == auth.account.id {
            return Err(IdentityError::OwnAccount);
        }

        let _changes = c.changes.lock().await;
        let now = c.now();
        let account = c.ports.accounts.find_by_id(id).await?.ok_or(IdentityError::AccountNotFound)?;
        let password = NewPassword::parse(password, &account.name)?;
        c.ports.system.set_password(&account.name, password.secret(), request).await?;
        let hash = c.ports.hasher.hash(password.secret()).await?;
        c.ports.accounts.reset_password(id, &hash, now).await?;
        c.ports.sessions.delete_all(id).await?;

        let entry = AuditEntry::new(now, AuditAction::PasswordReset.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(account.name.as_str())).await?;
        Ok(())
    }
}

/// Command: an administrator deletes another account, with its Linux user
/// and Samba entry. Its files stay on the volumes.
#[derive(Clone)]
pub struct DeleteAccount {
    context: Arc<Context>,
}

impl DeleteAccount {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(
        &self,
        auth: &AuthContext,
        id: AccountId,
        request: &RequestContext,
    ) -> Result<(), IdentityError> {
        let c = &self.context;
        Context::require(auth, Permission::UsersManage)?;
        if id == auth.account.id {
            return Err(IdentityError::OwnAccount);
        }

        let _changes = c.changes.lock().await;
        let now = c.now();
        let account = c.ports.accounts.find_by_id(id).await?.ok_or(IdentityError::AccountNotFound)?;
        let enabled_admin = account.role == Role::Admin && !account.disabled;
        if enabled_admin && !c.another_enabled_admin(id).await? {
            return Err(IdentityError::LastAdmin);
        }
        c.ports.system.delete_user(&account.name, request).await?;
        c.ports.accounts.delete(id).await?;

        let entry = AuditEntry::new(now, AuditAction::AccountDelete.as_str(), Outcome::Success, request);
        c.audit(entry.by(&auth.actor()).target(account.name.as_str())).await?;
        Ok(())
    }
}
