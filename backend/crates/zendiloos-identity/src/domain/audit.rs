/// What the Identity context writes to the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    Setup,
    Login,
    Logout,
    PasswordChange,
    AccountCreate,
    AccountUpdate,
    AccountDelete,
    PasswordReset,
    GroupCreate,
    GroupUpdate,
    GroupDelete,
    PasswordPolicyUpdate,
    SecurityUpdate,
    AddressBlock,
    AddressUnblock,
    SessionEnd,
}

impl AuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditAction::Setup => "identity.setup",
            AuditAction::Login => "identity.login",
            AuditAction::Logout => "identity.logout",
            AuditAction::PasswordChange => "identity.password_change",
            AuditAction::AccountCreate => "identity.account_create",
            AuditAction::AccountUpdate => "identity.account_update",
            AuditAction::AccountDelete => "identity.account_delete",
            AuditAction::PasswordReset => "identity.password_reset",
            AuditAction::GroupCreate => "identity.group_create",
            AuditAction::GroupUpdate => "identity.group_update",
            AuditAction::GroupDelete => "identity.group_delete",
            AuditAction::PasswordPolicyUpdate => "identity.password_policy_update",
            AuditAction::SecurityUpdate => "identity.security_update",
            AuditAction::AddressBlock => "identity.address_block",
            AuditAction::AddressUnblock => "identity.address_unblock",
            AuditAction::SessionEnd => "identity.session_end",
        }
    }
}
