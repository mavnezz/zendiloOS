//! Identity's commands on groups and accounts wired to the Sharing context
//! through the real bridges, with the in-memory fakes of both contexts:
//! what Samba is told, and whose connections end, when members or accounts
//! change; no file changes (docs/architecture.md, 4.4).

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_identity::application::commands::{AccountChanges, GroupChanges, NewGroupInput};
use zendiloos_identity::application::{
    AccountSummary, AuthContext, IdentityConfig, IdentityPorts, IdentityUseCases,
};
use zendiloos_identity::domain::{AccountId, AccountName, NewAccount, Role, SessionId};
use zendiloos_identity::testing::{
    FakeCodes, FakeHasher, FakeSystemAccounts, FakeTokens, InMemoryAccounts, InMemoryGroups,
    InMemoryPreferences, InMemorySecurity, InMemorySessions, InMemoryTwoFactor, ManualClock, MemoryAudit,
    MemorySetupCodes,
};
use zendiloos_jobs::Jobs;
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
use zendiloos_kernel::{AuditEntry, AuditLog, PortError, RequestContext, Secret, Timestamp};
use zendiloos_notify::Notifier;
use zendiloos_notify::testing::MemoryNotifications;
use zendiloos_sharing::application::ports::{Directory, Names};
use zendiloos_sharing::application::{NewShareInput, SharingUseCases};
use zendiloos_sharing::domain::{AccessRule, Permission, Principal, ShareSettings, SmbSpec, Subject};
use zendiloos_sharing::testing::{Files, Host, Projects, Removable, Shares, Volumes};

use super::{SharingFollowsAccounts, SharingFollowsGroups};

/// The Sharing context's view of identity's accounts and groups, as the
/// database gives it in production (`SqliteShares` reads the same tables).
struct FromIdentity {
    accounts: Arc<InMemoryAccounts>,
    groups: Arc<InMemoryGroups>,
}

#[async_trait]
impl Directory for FromIdentity {
    async fn names(&self) -> Result<Names, PortError> {
        let mut names = Names::default();
        for account in self.accounts.all() {
            names.accounts.insert(account.id.get(), account.name.to_string());
            if account.disabled {
                names.disabled.insert(account.id.get());
            }
        }
        for group in self.groups.all() {
            names.groups.insert(group.id.get(), group.name.to_string());
            names.members.insert(group.id.get(), group.members.iter().map(|id| id.get()).collect());
        }
        Ok(names)
    }

    async fn memberships(&self, account: i64) -> Result<Vec<i64>, PortError> {
        Ok(self
            .groups
            .all()
            .iter()
            .filter(|group| group.members.iter().any(|id| id.get() == account))
            .map(|group| group.id.get())
            .collect())
    }
}

struct NoAudit;

#[async_trait]
impl AuditLog for NoAudit {
    async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
        Ok(())
    }
}

struct World {
    identity: IdentityUseCases,
    sharing: SharingUseCases,
    host: Arc<Host>,
    jobs: Jobs,
    admin: AuthContext,
}

fn world() -> World {
    let accounts = Arc::new(InMemoryAccounts::default());
    let groups = Arc::new(InMemoryGroups::default());
    let host = Arc::new(Host::default());
    let shares = Arc::new(Shares::default());
    let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
    let ports = zendiloos_sharing::application::SharingPorts {
        shares: shares.clone(),
        directory: Arc::new(FromIdentity { accounts: accounts.clone(), groups: groups.clone() }),
        volumes: Arc::new(Volumes),
        projects: Arc::new(Projects::default()),
        host: host.clone(),
        files: Arc::new(Files::default()),
        removable: Arc::new(Removable::default()),
        jobs: jobs.clone(),
        audit: Arc::new(NoAudit),
        clock: Arc::new(ManualClock::default()),
        notifier: Notifier::new(Arc::new(MemoryNotifications::default()), Arc::new(FixedClock)),
    };
    let sharing = SharingUseCases::new(ports.clone());
    let identity = IdentityUseCases::new(
        IdentityPorts {
            accounts: accounts.clone(),
            groups: groups.clone(),
            group_observer: Arc::new(SharingFollowsGroups::new(sharing.clone())),
            account_observer: Arc::new(SharingFollowsAccounts::new(sharing.clone())),
            sessions: Arc::new(InMemorySessions::default()),
            security: Arc::new(InMemorySecurity::default()),
            preferences: Arc::new(InMemoryPreferences::default()),
            system: Arc::new(FakeSystemAccounts::default()),
            hasher: Arc::new(FakeHasher::default()),
            tokens: Arc::new(FakeTokens::default()),
            setup_codes: Arc::new(MemorySetupCodes::default()),
            two_factor: Arc::new(InMemoryTwoFactor::default()),
            codes: Arc::new(FakeCodes::default()),
            audit: Arc::new(MemoryAudit::default()),
            clock: Arc::new(ManualClock::default()),
        },
        IdentityConfig::default(),
    );
    let add = |name: &str, role| {
        accounts.insert(NewAccount {
            name: AccountName::parse(name).unwrap(),
            full_name: None,
            description: None,
            email: None,
            role,
            password_hash: FakeHasher::hash_of("correct horse battery"),
            password_change_required: false,
            password_locked: false,
            created_at: Timestamp::from_unix_seconds(1_800_000_000),
        })
    };
    let alice = add("alice", Role::Admin);
    add("bob", Role::User);
    let admin = AuthContext {
        session_id: SessionId::new(1),
        csrf_token: Secret::new("csrf".to_owned()),
        expires_at: Timestamp::from_unix_seconds(1_900_000_000),
        account: AccountSummary {
            id: alice.id,
            name: alice.name.clone(),
            full_name: None,
            role: Role::Admin,
            password_change_required: false,
            password_locked: false,
            created_at: alice.created_at,
        },
    };
    World { identity, sharing, host, jobs, admin }
}

const ALICE: i64 = 1;
const BOB: i64 = 2;

fn rule(subject: Subject, permission: Permission) -> AccessRule {
    AccessRule { subject, permission }
}

fn share(name: &str, access: Vec<AccessRule>) -> NewShareInput {
    NewShareInput {
        name: name.into(),
        description: None,
        pool: "tank".into(),
        shared: true,
        recycle_bin: true,
        access,
        nfs: vec![],
        settings: ShareSettings::default(),
    }
}

fn group(name: &str, members: Vec<AccountId>) -> NewGroupInput {
    NewGroupInput { name: name.into(), description: None, members }
}

/// The last Samba section the fake host was told of for `share`.
fn section(host: &Host, share: &str) -> Option<SmbSpec> {
    let configs = host.configs.lock().unwrap();
    configs.last().and_then(|config| config.smb.iter().find(|spec| spec.name.as_str() == share).cloned())
}

/// What the fake host was told from call `from` on.
fn told(host: &Host, from: usize) -> Vec<String> {
    host.calls()[from..].to_vec()
}

/// bob may write `photos` by his own rule; `kids` is denied there. An
/// administrator adds bob to `kids`: Samba, which follows the group itself,
/// keeps him out on his next connection, and his open ones end now.
#[tokio::test]
async fn a_new_member_of_a_denied_group_is_out_at_once() {
    let w = world();
    let request = RequestContext::default();
    let kids = w.identity.create_group.execute(&w.admin, group("kids", vec![]), &request).await.unwrap();
    let rules = vec![
        rule(Subject::Account(BOB), Permission::ReadWrite),
        rule(Subject::Group(kids.id.get()), Permission::Deny),
    ];
    w.sharing.create(share("photos", rules), &w.admin.actor(), &request).await.unwrap();
    let before = w.host.calls().len();

    let changes = GroupChanges { members: Some(vec![AccountId::new(BOB)]), description: None };
    let kids = w.identity.update_group.execute(&w.admin, kids.id, changes, &request).await.unwrap();

    assert_eq!(kids.members, [AccountId::new(BOB)]);
    assert_eq!(told(&w.host, before), ["apply 1", "close photos"]);
    let photos = section(&w.host, "photos").unwrap();
    assert_eq!(
        photos.access,
        [
            (Principal::User("bob".into()), Permission::ReadWrite),
            (Principal::Group("kids".into()), Permission::Deny)
        ],
        "the group as a group: Samba follows its members"
    );
    assert!(w.jobs.running().is_empty(), "no walk");
}

/// bob writes `docs` as a member of `writers` and reads it as one of
/// `readers`; he leaves `writers`, and may only read from his next
/// connection on. Joining a group that gives more ends nothing.
#[tokio::test]
async fn leaving_a_writing_group_ends_the_connections_joining_one_does_not() {
    let w = world();
    let request = RequestContext::default();
    let bob = AccountId::new(BOB);
    let writers =
        w.identity.create_group.execute(&w.admin, group("writers", vec![]), &request).await.unwrap();
    let readers =
        w.identity.create_group.execute(&w.admin, group("readers", vec![bob]), &request).await.unwrap();
    let rules = vec![
        rule(Subject::Group(writers.id.get()), Permission::ReadWrite),
        rule(Subject::Group(readers.id.get()), Permission::Read),
    ];
    w.sharing.create(share("docs", rules), &w.admin.actor(), &request).await.unwrap();

    let before = w.host.calls().len();
    let join = GroupChanges { members: Some(vec![bob]), description: None };
    w.identity.update_group.execute(&w.admin, writers.id, join, &request).await.unwrap();
    assert_eq!(told(&w.host, before), ["apply 1"], "bob gains: nothing to end");

    let before = w.host.calls().len();
    let leave = GroupChanges { members: Some(vec![]), description: None };
    w.identity.update_group.execute(&w.admin, writers.id, leave, &request).await.unwrap();
    assert_eq!(told(&w.host, before), ["apply 1", "close docs"], "bob may only read now");
    assert!(w.jobs.running().is_empty(), "nothing in the files changes");
}

/// bob, who may write `photos`, is disabled: Samba denies him by name
/// there, besides having him disabled, and his open connections end; the
/// share that never named him is left alone. Enabled again, he writes.
#[tokio::test]
async fn a_disabled_account_is_denied_on_the_shares_it_could_use() {
    let w = world();
    let request = RequestContext::default();
    let rules = vec![
        rule(Subject::Account(ALICE), Permission::ReadWrite),
        rule(Subject::Account(BOB), Permission::ReadWrite),
    ];
    w.sharing.create(share("photos", rules), &w.admin.actor(), &request).await.unwrap();
    let media = vec![rule(Subject::Account(ALICE), Permission::Read)];
    w.sharing.create(share("media", media), &w.admin.actor(), &request).await.unwrap();
    let before = w.host.calls().len();

    let disable = AccountChanges { disabled: Some(true), ..AccountChanges::default() };
    let bob =
        w.identity.update_account.execute(&w.admin, AccountId::new(BOB), disable, &request).await.unwrap();

    assert!(bob.disabled);
    assert_eq!(told(&w.host, before), ["apply 2", "close photos"], "media never named him");
    let photos = section(&w.host, "photos").unwrap();
    assert!(
        photos.access.contains(&(Principal::User("bob".into()), Permission::Deny)),
        "{:?}",
        photos.access
    );

    let before = w.host.calls().len();
    let enable = AccountChanges { disabled: Some(false), ..AccountChanges::default() };
    w.identity.update_account.execute(&w.admin, AccountId::new(BOB), enable, &request).await.unwrap();
    assert_eq!(told(&w.host, before), ["apply 2"]);
    let photos = section(&w.host, "photos").unwrap();
    assert!(photos.access.contains(&(Principal::User("bob".into()), Permission::ReadWrite)));
    assert!(w.jobs.running().is_empty(), "no walk");
}

/// bob is deleted: the shares he could use end his connections, and Samba
/// names him no more.
#[tokio::test]
async fn a_deleted_accounts_connections_end_and_its_name_goes() {
    let w = world();
    let request = RequestContext::default();
    let rules = vec![
        rule(Subject::Account(ALICE), Permission::ReadWrite),
        rule(Subject::Account(BOB), Permission::ReadWrite),
    ];
    w.sharing.create(share("photos", rules), &w.admin.actor(), &request).await.unwrap();
    let before = w.host.calls().len();

    w.identity.delete_account.execute(&w.admin, AccountId::new(BOB), &request).await.unwrap();

    assert_eq!(told(&w.host, before), ["apply 1", "close photos"]);
    let photos = section(&w.host, "photos").unwrap();
    assert_eq!(photos.access, [(Principal::User("alice".into()), Permission::ReadWrite)]);
}

/// A group is deleted: its rules go first, as a change of each share's
/// rules, and its members' connections end where it let them in.
#[tokio::test]
async fn a_group_takes_its_rules_along() {
    let w = world();
    let request = RequestContext::default();
    let family =
        w.identity.create_group.execute(&w.admin, group("family", vec![AccountId::new(BOB)]), &request).await;
    let family = family.unwrap();
    let rules = vec![
        rule(Subject::Account(ALICE), Permission::ReadWrite),
        rule(Subject::Group(family.id.get()), Permission::ReadWrite),
    ];
    w.sharing.create(share("photos", rules), &w.admin.actor(), &request).await.unwrap();
    let before = w.host.calls().len();

    w.identity.delete_group.execute(&w.admin, family.id, &request).await.unwrap();

    assert_eq!(told(&w.host, before), ["apply 1", "close photos"]);
    let photos = section(&w.host, "photos").unwrap();
    assert_eq!(photos.access, [(Principal::User("alice".into()), Permission::ReadWrite)]);
}
