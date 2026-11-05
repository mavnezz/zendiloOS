use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{AgentTransport, ErrorCode, GroupName, Password, Request, Response, UserName};
use zendiloos_identity::application::ports::{Creation, SystemAccounts};
use zendiloos_identity::domain::{self as identity, AccountName};
use zendiloos_kernel::{PortError, RequestContext, Secret};

use super::{WHAT, agent_error, call_raw, expect_done, unexpected};

/// [`SystemAccounts`] through the agent.
pub struct AgentSystemAccounts {
    transport: Arc<dyn AgentTransport>,
}

impl AgentSystemAccounts {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }

    /// `conflict` from the agent means the name is taken.
    async fn create(&self, request: Request, context: &RequestContext) -> Result<Creation, PortError> {
        match call_raw(self.transport.as_ref(), request, context).await? {
            Response::Done => Ok(Creation::Created),
            Response::Error { code: ErrorCode::Conflict, .. } => Ok(Creation::NameTaken),
            Response::Error { code, message } => Err(agent_error(code, &message)),
            other => Err(unexpected(other)),
        }
    }

    async fn done(&self, request: Request, context: &RequestContext) -> Result<(), PortError> {
        expect_done(self.transport.as_ref(), request, context).await
    }
}

// The names follow the same rules on both sides; a failure here is a bug.
fn user(name: &AccountName) -> Result<UserName, PortError> {
    UserName::parse(name.as_str()).map_err(|e| PortError::new(WHAT, e))
}

fn group(name: &identity::GroupName) -> Result<GroupName, PortError> {
    GroupName::parse(name.as_str()).map_err(|e| PortError::new(WHAT, e))
}

#[async_trait]
impl SystemAccounts for AgentSystemAccounts {
    async fn create_user(&self, name: &AccountName, context: &RequestContext) -> Result<Creation, PortError> {
        self.create(Request::CreateUser { name: user(name)? }, context).await
    }

    async fn delete_user(&self, name: &AccountName, context: &RequestContext) -> Result<(), PortError> {
        self.done(Request::DeleteUser { name: user(name)? }, context).await
    }

    async fn set_password(
        &self,
        name: &AccountName,
        password: &Secret,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        let password = Password::new(password.expose()).map_err(|e| PortError::new(WHAT, e))?;
        self.done(Request::SetSmbPassword { name: user(name)?, password }, context).await
    }

    async fn set_enabled(
        &self,
        name: &AccountName,
        enabled: bool,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        self.done(Request::SetUserEnabled { name: user(name)?, enabled }, context).await
    }

    async fn create_group(
        &self,
        name: &identity::GroupName,
        context: &RequestContext,
    ) -> Result<Creation, PortError> {
        self.create(Request::CreateGroup { name: group(name)? }, context).await
    }

    async fn delete_group(
        &self,
        name: &identity::GroupName,
        context: &RequestContext,
    ) -> Result<(), PortError> {
        self.done(Request::DeleteGroup { name: group(name)? }, context).await
    }

    async fn set_group_members(
        &self,
        name: &identity::GroupName,
        members: &[AccountName],
        context: &RequestContext,
    ) -> Result<(), PortError> {
        let members = members.iter().map(user).collect::<Result<_, _>>()?;
        self.done(Request::SetGroupMembers { group: group(name)?, members }, context).await
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    fn name(value: &str) -> AccountName {
        AccountName::parse(value).unwrap()
    }

    #[tokio::test]
    async fn accounts_become_agent_requests() {
        let agent = Arc::new(SimulatedAgent::new());
        let system = AgentSystemAccounts::new(agent.clone());
        let context = RequestContext::default();

        assert_eq!(system.create_user(&name("alice"), &context).await.unwrap(), Creation::Created);
        system.set_password(&name("alice"), &Secret::new("correct horse battery"), &context).await.unwrap();
        let family = identity::GroupName::parse("family").unwrap();
        assert_eq!(system.create_group(&family, &context).await.unwrap(), Creation::Created);
        system.set_group_members(&family, &[name("alice")], &context).await.unwrap();

        let ops: Vec<_> = agent.received().iter().map(Request::name).collect();
        assert_eq!(ops, ["create_user", "set_smb_password", "create_group", "set_group_members"]);
    }

    #[tokio::test]
    async fn taken_names_are_reported_as_such() {
        let system = AgentSystemAccounts::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        assert_eq!(system.create_user(&name("debian"), &context).await.unwrap(), Creation::NameTaken);
        assert_eq!(system.create_user(&name("alice"), &context).await.unwrap(), Creation::Created);
        assert_eq!(system.create_user(&name("alice"), &context).await.unwrap(), Creation::NameTaken);
    }
}
