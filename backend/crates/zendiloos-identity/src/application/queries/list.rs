use std::sync::Arc;

use crate::application::context::Context;
use crate::application::{AuthContext, IdentityError};
use crate::domain::{Account, Group, Permission};

/// Query: all accounts, for administrators.
#[derive(Clone)]
pub struct ListAccounts {
    context: Arc<Context>,
}

impl ListAccounts {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext) -> Result<Vec<Account>, IdentityError> {
        Context::require(auth, Permission::UsersManage)?;
        Ok(self.context.ports.accounts.list().await?)
    }
}

/// Query: all groups, for administrators.
#[derive(Clone)]
pub struct ListGroups {
    context: Arc<Context>,
}

impl ListGroups {
    pub(crate) fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn execute(&self, auth: &AuthContext) -> Result<Vec<Group>, IdentityError> {
        Context::require(auth, Permission::UsersManage)?;
        Ok(self.context.ports.groups.list().await?)
    }
}
