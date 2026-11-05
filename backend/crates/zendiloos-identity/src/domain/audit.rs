/// What the Identity context writes to the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    Setup,
    Login,
    Logout,
    PasswordChange,
}

impl AuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditAction::Setup => "identity.setup",
            AuditAction::Login => "identity.login",
            AuditAction::Logout => "identity.logout",
            AuditAction::PasswordChange => "identity.password_change",
        }
    }
}
