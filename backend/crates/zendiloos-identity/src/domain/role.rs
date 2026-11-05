/// Role of an account. Roles are fixed and bundle permissions; code asks for
/// a [`Permission`], never for a role, so the model can change in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Full control over the system.
    Admin,
    /// Uses shares and the file manager; sees the dashboard.
    User,
    /// Sees everything, changes nothing.
    ReadOnly,
}

/// Something a request wants to do. The grants follow
/// docs/architecture.md, section 4.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Dashboard and system information.
    SystemView,
    /// Settings, power, updates.
    SystemManage,
    /// Logs can contain user names and addresses.
    LogsView,
    UsersManage,
    StorageView,
    /// Creating and destroying pools; wipes disks.
    StorageManage,
    SharesManage,
    /// Own shares in the file manager, reading.
    FilesRead,
    /// Own shares in the file manager, changing.
    FilesWrite,
    ContainersManage,
    /// Own password, sessions and preferences.
    AccountSelf,
}

impl Permission {
    pub const ALL: [Permission; 11] = [
        Permission::SystemView,
        Permission::SystemManage,
        Permission::LogsView,
        Permission::UsersManage,
        Permission::StorageView,
        Permission::StorageManage,
        Permission::SharesManage,
        Permission::FilesRead,
        Permission::FilesWrite,
        Permission::ContainersManage,
        Permission::AccountSelf,
    ];

    /// Stable name, as documented and as sent to the frontend.
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::SystemView => "system.view",
            Permission::SystemManage => "system.manage",
            Permission::LogsView => "logs.view",
            Permission::UsersManage => "users.manage",
            Permission::StorageView => "storage.view",
            Permission::StorageManage => "storage.manage",
            Permission::SharesManage => "shares.manage",
            Permission::FilesRead => "files.read",
            Permission::FilesWrite => "files.write",
            Permission::ContainersManage => "containers.manage",
            Permission::AccountSelf => "account.self",
        }
    }
}

impl Role {
    /// Every permission this role grants.
    pub fn permissions(self) -> impl Iterator<Item = Permission> {
        Permission::ALL.into_iter().filter(move |p| self.grants(*p))
    }

    pub fn grants(self, permission: Permission) -> bool {
        use Permission::*;
        match self {
            Role::Admin => true,
            Role::User => matches!(permission, SystemView | FilesRead | FilesWrite | AccountSelf),
            Role::ReadOnly => {
                matches!(permission, SystemView | LogsView | StorageView | FilesRead | AccountSelf)
            }
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
            Role::ReadOnly => "readonly",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            "readonly" => Some(Role::ReadOnly),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Permission::*;
    use super::*;

    fn granted(role: Role) -> Vec<Permission> {
        role.permissions().collect()
    }

    #[test]
    fn grants_match_the_documented_table() {
        assert_eq!(granted(Role::Admin), Permission::ALL);
        assert_eq!(granted(Role::User), [SystemView, FilesRead, FilesWrite, AccountSelf]);
        assert_eq!(granted(Role::ReadOnly), [SystemView, LogsView, StorageView, FilesRead, AccountSelf]);
    }

    #[test]
    fn permission_names_are_unique() {
        let mut names: Vec<&str> = Permission::ALL.iter().map(|p| p.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Permission::ALL.len());
    }

    #[test]
    fn names_round_trip() {
        for role in [Role::Admin, Role::User, Role::ReadOnly] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        assert_eq!(Role::parse("root"), None);
    }
}
