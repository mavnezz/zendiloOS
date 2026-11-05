//! Accounts and groups, managed by administrators (`users.manage`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zendiloos_identity::application::commands::{
    AccountChanges, GroupChanges, NewAccountInput, NewGroupInput,
};
use zendiloos_identity::domain::{self as identity, AccountId, GroupId, Role};
use zendiloos_kernel::Secret;

use super::identity::AccountRole;
use crate::auth::Auth;
use crate::error::{ApiError, Problem};
use crate::extract::{ApiJson, Client};
use crate::state::ApiState;

pub(super) fn routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(list_accounts, create_account))
        .routes(routes!(update_account, delete_account))
        .routes(routes!(reset_password))
        .routes(routes!(list_groups, create_group))
        .routes(routes!(update_group, delete_group))
}

impl From<AccountRole> for Role {
    fn from(role: AccountRole) -> Self {
        match role {
            AccountRole::Admin => Role::Admin,
            AccountRole::User => Role::User,
            AccountRole::Readonly => Role::ReadOnly,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Account {
    pub id: i64,
    /// Login name, also the Linux and SMB user name.
    pub name: String,
    pub full_name: Option<String>,
    pub role: AccountRole,
    pub disabled: bool,
    /// The account must choose a new password at its next login.
    pub password_change_required: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<identity::Account> for Account {
    fn from(account: identity::Account) -> Self {
        Self {
            id: account.id.get(),
            name: account.name.to_string(),
            full_name: account.full_name,
            role: account.role.into(),
            disabled: account.disabled,
            password_change_required: account.password_change_required,
            created_at: account.created_at.unix_seconds(),
            updated_at: account.updated_at.unix_seconds(),
        }
    }
}

/// All accounts, ordered by name.
#[utoipa::path(
    get,
    path = "/accounts",
    tag = "accounts",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<Account>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn list_accounts(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<Account>>, ApiError> {
    let accounts = state.identity.list_accounts.execute(&auth.0).await?;
    Ok(Json(accounts.into_iter().map(Account::from).collect()))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAccountRequest {
    pub name: String,
    pub full_name: Option<String>,
    pub role: AccountRole,
    /// Initial password; the account must change it at its first login.
    #[schema(format = Password)]
    pub password: String,
}

/// Creates an account with its Linux user and SMB password.
#[utoipa::path(
    post,
    path = "/accounts",
    tag = "accounts",
    security(("session" = [])),
    request_body = CreateAccountRequest,
    responses(
        (status = CREATED, body = Account),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
    )
)]
async fn create_account(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<CreateAccountRequest>,
) -> Result<(StatusCode, Json<Account>), ApiError> {
    let input = NewAccountInput {
        name: request.name,
        full_name: request.full_name,
        role: request.role.into(),
        password: Secret::new(request.password),
    };
    let account = state.identity.create_account.execute(&auth.0, input, &client).await?;
    Ok((StatusCode::CREATED, Json(account.into())))
}

/// Absent fields stay as they are.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAccountRequest {
    /// An empty string removes the full name.
    pub full_name: Option<String>,
    pub role: Option<AccountRole>,
    pub disabled: Option<bool>,
}

/// Changes another account's full name, role or whether it is enabled.
/// Disabling ends its sessions and SMB access.
#[utoipa::path(
    patch,
    path = "/accounts/{id}",
    tag = "accounts",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = UpdateAccountRequest,
    responses(
        (status = OK, body = Account),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`last_admin`, `own_account`"),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`, `csrf_rejected`"),
    )
)]
async fn update_account(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<UpdateAccountRequest>,
) -> Result<Json<Account>, ApiError> {
    let changes = AccountChanges {
        full_name: request.full_name.map(Some),
        role: request.role.map(Role::from),
        disabled: request.disabled,
    };
    let account =
        state.identity.update_account.execute(&auth.0, AccountId::new(id), changes, &client).await?;
    Ok(Json(account.into()))
}

/// Deletes another account with its Linux user. Its files stay.
#[utoipa::path(
    delete,
    path = "/accounts/{id}",
    tag = "accounts",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT, description = "Deleted."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`last_admin`, `own_account`"),
    )
)]
async fn delete_account(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.identity.delete_account.execute(&auth.0, AccountId::new(id), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetPasswordRequest {
    #[schema(format = Password)]
    pub password: String,
}

/// Sets a new password for another account, which must change it at its
/// next login. Ends its sessions.
#[utoipa::path(
    post,
    path = "/accounts/{id}/password",
    tag = "accounts",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = ResetPasswordRequest,
    responses(
        (status = NO_CONTENT, description = "Set."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
        (status = CONFLICT, body = Problem, description = "`own_account`"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn reset_password(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<ResetPasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let password = Secret::new(request.password);
    state.identity.reset_password.execute(&auth.0, AccountId::new(id), password, &client).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Group {
    pub id: i64,
    /// Also the Linux group name.
    pub name: String,
    pub description: Option<String>,
    /// Account IDs, ascending.
    pub members: Vec<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<identity::Group> for Group {
    fn from(group: identity::Group) -> Self {
        Self {
            id: group.id.get(),
            name: group.name.to_string(),
            description: group.description,
            members: group.members.into_iter().map(AccountId::get).collect(),
            created_at: group.created_at.unix_seconds(),
            updated_at: group.updated_at.unix_seconds(),
        }
    }
}

/// All groups, ordered by name.
#[utoipa::path(
    get,
    path = "/groups",
    tag = "accounts",
    security(("session" = [])),
    responses(
        (status = OK, body = Vec<Group>),
        (status = FORBIDDEN, body = Problem, description = "`forbidden`"),
    )
)]
async fn list_groups(auth: Auth, State(state): State<ApiState>) -> Result<Json<Vec<Group>>, ApiError> {
    let groups = state.identity.list_groups.execute(&auth.0).await?;
    Ok(Json(groups.into_iter().map(Group::from).collect()))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
    /// Account IDs.
    #[serde(default)]
    pub members: Vec<i64>,
}

/// Creates a group with its Linux group.
#[utoipa::path(
    post,
    path = "/groups",
    tag = "accounts",
    security(("session" = [])),
    request_body = CreateGroupRequest,
    responses(
        (status = CREATED, body = Group),
        (status = CONFLICT, body = Problem, description = "`name_taken`"),
        (status = NOT_FOUND, body = Problem, description = "`not_found`: a member does not exist"),
        (status = UNPROCESSABLE_ENTITY, body = Problem, description = "`invalid_input`"),
    )
)]
async fn create_group(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    ApiJson(request): ApiJson<CreateGroupRequest>,
) -> Result<(StatusCode, Json<Group>), ApiError> {
    let input = NewGroupInput {
        name: request.name,
        description: request.description,
        members: request.members.into_iter().map(AccountId::new).collect(),
    };
    let group = state.identity.create_group.execute(&auth.0, input, &client).await?;
    Ok((StatusCode::CREATED, Json(group.into())))
}

/// Absent fields stay as they are.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateGroupRequest {
    /// An empty string removes the description.
    pub description: Option<String>,
    /// Replaces all members.
    pub members: Option<Vec<i64>>,
}

/// Changes a group's description or members.
#[utoipa::path(
    patch,
    path = "/groups/{id}",
    tag = "accounts",
    security(("session" = [])),
    params(("id" = i64, Path)),
    request_body = UpdateGroupRequest,
    responses(
        (status = OK, body = Group),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn update_group(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
    ApiJson(request): ApiJson<UpdateGroupRequest>,
) -> Result<Json<Group>, ApiError> {
    let changes = GroupChanges {
        description: request.description.map(Some),
        members: request.members.map(|ids| ids.into_iter().map(AccountId::new).collect()),
    };
    let group = state.identity.update_group.execute(&auth.0, GroupId::new(id), changes, &client).await?;
    Ok(Json(group.into()))
}

/// Deletes a group with its Linux group.
#[utoipa::path(
    delete,
    path = "/groups/{id}",
    tag = "accounts",
    security(("session" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = NO_CONTENT, description = "Deleted."),
        (status = NOT_FOUND, body = Problem, description = "`not_found`"),
    )
)]
async fn delete_group(
    auth: Auth,
    State(state): State<ApiState>,
    Client(client): Client,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.identity.delete_group.execute(&auth.0, GroupId::new(id), &client).await?;
    Ok(StatusCode::NO_CONTENT)
}
