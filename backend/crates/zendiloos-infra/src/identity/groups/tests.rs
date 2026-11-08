//! Identity's commands on groups and accounts wired to the Sharing context
//! through the real bridges, with the in-memory fakes of both contexts: what
//! the shares' folders and files are given when members, accounts or
//! snapshots change (docs/architecture.md, 4.4).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

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
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
use zendiloos_jobs::{JobState, Jobs};
use zendiloos_kernel::{AuditEntry, AuditLog, PortError, RequestContext, Secret, Timestamp};
use zendiloos_notify::Notifier;
use zendiloos_notify::testing::MemoryNotifications;
use zendiloos_sharing::application::ports::{Directory, Names};
use zendiloos_sharing::application::{NewShareInput, ShareChanges, SharingPorts, SharingUseCases};
use zendiloos_sharing::domain::{Access, AccessRule, Permission, ShareId, ShareSettings, Subject};
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
    ports: SharingPorts,
    shares: Arc<Shares>,
    host: Arc<Host>,
    jobs: Jobs,
    clock: Arc<ManualClock>,
    admin: AuthContext,
}

fn world() -> World {
    let accounts = Arc::new(InMemoryAccounts::default());
    let groups = Arc::new(InMemoryGroups::default());
    let host = Arc::new(Host::default());
    let shares = Arc::new(Shares::default());
    let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
    let clock = Arc::new(ManualClock::default());
    let ports = SharingPorts {
        shares: shares.clone(),
        directory: Arc::new(FromIdentity { accounts: accounts.clone(), groups: groups.clone() }),
        volumes: Arc::new(Volumes),
        projects: Arc::new(Projects::default()),
        host: host.clone(),
        files: Arc::new(Files::default()),
        removable: Arc::new(Removable::default()),
        jobs: jobs.clone(),
        audit: Arc::new(NoAudit),
        clock: clock.clone(),
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
    World { identity, sharing, ports, shares, host, jobs, clock, admin }
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

async fn finished(jobs: &Jobs, id: zendiloos_jobs::JobId) -> JobState {
    for _ in 0..200 {
        let current = jobs.find(id).await.unwrap().unwrap();
        if current.state != JobState::Running {
            return current.state;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("job did not finish");
}

/// Waits until no job runs any more: the walks queued in the background.
async fn all_finished(jobs: &Jobs) {
    for _ in 0..200 {
        if jobs.running().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("jobs did not finish: {:?}", jobs.running());
}

/// The ACL the fake host was last told to give `share`'s folder (a walk
/// gives the folder its rules first, too), and the last it was told to
/// give the whole tree. Failed walks gave nothing.
fn last_acls(host: &Host, share: &str) -> (Option<String>, Option<String>) {
    let calls = host.calls();
    let prefix = format!("access 1 {share} ");
    let folder = calls.iter().rev().find(|c| c.starts_with(&prefix)).cloned();
    let tree = calls.iter().rev().find(|c| c.starts_with(&prefix) && c.ends_with(" recursive")).cloned();
    (folder, tree)
}

/// bob may write `photos` by his own rule; `kids` is denied there. While a
/// walk of `photos` is under way (any rule change starts one, which may
/// take hours), an administrator adds bob to `kids`. The change stands and
/// is not refused; the share's folder and files deny bob once the walk
/// under way has ended, and the answer says photos follows.
#[tokio::test]
async fn a_denial_by_a_new_group_membership_is_never_lost() {
    let w = world();
    let request = RequestContext::default();
    let kids = w.identity.create_group.execute(&w.admin, group("kids", vec![]), &request).await.unwrap();
    let photos = w
        .sharing
        .create(
            share(
                "photos",
                vec![
                    rule(Subject::Account(BOB), Permission::ReadWrite),
                    rule(Subject::Group(kids.id.get()), Permission::Deny),
                ],
            ),
            &w.admin.actor(),
            &request,
        )
        .await
        .unwrap();

    // Any change of photos' rules starts a walk; it is held in the "agent".
    w.host.hold_walks.store(true, Ordering::Relaxed);
    let reader = ShareChanges {
        access: Some(vec![
            rule(Subject::Account(BOB), Permission::ReadWrite),
            rule(Subject::Group(kids.id.get()), Permission::Deny),
            rule(Subject::Account(ALICE), Permission::Read),
        ]),
        ..ShareChanges::default()
    };
    let (_, walk) = w.sharing.update(photos.id, reader, &w.admin.actor(), &request).await.unwrap();
    while w.host.walks_held.load(Ordering::Relaxed) == 0 {
        tokio::task::yield_now().await;
    }

    // The administrator adds bob to kids.
    let changes = GroupChanges { members: Some(vec![AccountId::new(BOB)]), description: None };
    let (stored, pending) =
        w.identity.update_group.execute(&w.admin, kids.id, changes, &request).await.unwrap();
    assert_eq!(stored.members, [AccountId::new(BOB)], "the change stands");
    assert_eq!(pending, ["photos"], "and says what follows in the background");

    // Let the walk finish; the one after it gives the new members.
    w.host.hold_walks.store(false, Ordering::Relaxed);
    w.host.let_walks_on.notify_one();
    assert_eq!(finished(&w.jobs, walk.unwrap().id).await, JobState::Succeeded);
    all_finished(&w.jobs).await;

    let seen = w.sharing.subject_access(Subject::Account(BOB)).await.unwrap();
    assert_eq!(seen[0].effective, None, "the database denies bob");
    let (folder, tree) = last_acls(&w.host, "photos");
    assert!(
        folder.as_deref().is_some_and(|f| f.contains("bob:-"))
            && tree.as_deref().is_some_and(|t| t.contains("bob:-")),
        "the folder and files deny bob as well: folder {folder:?}, tree {tree:?}"
    );
    assert!(!w.shares.owes(photos.id));
}

/// `kids` may write `albums` and is denied `photos`; the host refuses
/// every change while bob joins kids (a share not mounted, the files agent
/// waited 15 s for a walk, …). Identity reports success, and both shares
/// stay owed: a restart of zendiloosd walks them.
#[tokio::test]
async fn a_refusal_on_one_share_does_not_cost_the_denial_on_another() {
    let w = world();
    let request = RequestContext::default();
    let actor = w.admin.actor();
    let kids = w.identity.create_group.execute(&w.admin, group("kids", vec![]), &request).await.unwrap();
    let kid = Subject::Group(kids.id.get());
    let albums = share("albums", vec![rule(kid, Permission::ReadWrite)]);
    let albums = w.sharing.create(albums, &actor, &request).await.unwrap().id;
    let photos = share(
        "photos",
        vec![rule(Subject::Account(BOB), Permission::ReadWrite), rule(kid, Permission::Deny)],
    );
    let photos = w.sharing.create(photos, &actor, &request).await.unwrap().id;
    w.host.refuse_access.store(true, Ordering::Relaxed);

    let changes = GroupChanges { members: Some(vec![AccountId::new(BOB)]), description: None };
    let (_, pending) = w.identity.update_group.execute(&w.admin, kids.id, changes, &request).await.unwrap();
    assert_eq!(pending, ["photos", "albums"], "every share is tried, none stops the others");
    all_finished(&w.jobs).await;
    assert!(w.shares.owes(photos) && w.shares.owes(albums), "nothing was given: both still owed");
    w.host.refuse_access.store(false, Ordering::Relaxed);

    // zendiloosd starts again: what is owed is in the database.
    let restarted = SharingUseCases::new(w.ports.clone());
    assert_eq!(restarted.reconcile_access(&request).await.unwrap().len(), 2);
    all_finished(&w.jobs).await;

    let (folder, tree) = last_acls(&w.host, "photos");
    assert!(folder.as_deref().is_some_and(|f| f.contains("bob:-")), "{folder:?}");
    assert!(tree.as_deref().is_some_and(|t| t.contains("bob:-")), "{tree:?}");
    let (_, tree) = last_acls(&w.host, "albums");
    assert!(tree.is_some(), "albums' walk makes bob's recycle bin");
    assert!(!w.shares.owes(photos) && !w.shares.owes(albums));
}

/// `writers` may write `docs`, `readers` may read it; bob is in both,
/// writes files there (they are his), then leaves `writers`. What someone
/// who may no longer write owns passes to root (docs 4.4 Owners): his
/// leaving walks the share.
#[tokio::test]
async fn leaving_a_writing_group_hands_over_what_one_owns() {
    let w = world();
    let request = RequestContext::default();
    let bob = || vec![AccountId::new(BOB)];
    let writers = w.identity.create_group.execute(&w.admin, group("writers", bob()), &request).await.unwrap();
    let readers = w.identity.create_group.execute(&w.admin, group("readers", bob()), &request).await.unwrap();
    let rules = vec![
        rule(Subject::Group(writers.id.get()), Permission::ReadWrite),
        rule(Subject::Group(readers.id.get()), Permission::Read),
    ];
    let docs: ShareId = w.sharing.create(share("docs", rules), &w.admin.actor(), &request).await.unwrap().id;
    let before = w.host.calls().len();

    let changes = GroupChanges { members: Some(vec![]), description: None };
    let (_, pending) =
        w.identity.update_group.execute(&w.admin, writers.id, changes, &request).await.unwrap();
    assert_eq!(pending, ["docs"]);
    all_finished(&w.jobs).await;

    let seen = w.sharing.subject_access(Subject::Account(BOB)).await.unwrap();
    let share = seen.iter().find(|s| s.share == docs).unwrap();
    assert_eq!(share.effective, Some(Access::Read), "bob reads only");
    let after: Vec<String> = w.host.calls()[before..].to_vec();
    assert!(
        after.iter().any(|call| call.starts_with("access 1 docs ") && call.ends_with(" recursive")),
        "a walk of docs, which hands over what bob owns and his recycle bin: {after:?}"
    );
    assert!(!w.shares.owes(docs));
}

/// `staff` may write, `interns` is denied; bob is in both when the share
/// is made. A group entry without permissions denies nobody in POSIX ACLs:
/// the new share's folder denies bob himself, as for every later change.
#[tokio::test]
async fn a_share_made_with_a_denied_group_denies_its_members() {
    let w = world();
    let request = RequestContext::default();
    let bob = || vec![AccountId::new(BOB)];
    let staff = w.identity.create_group.execute(&w.admin, group("staff", bob()), &request).await.unwrap();
    let interns = w.identity.create_group.execute(&w.admin, group("interns", bob()), &request).await.unwrap();
    let rules = vec![
        rule(Subject::Group(staff.id.get()), Permission::ReadWrite),
        rule(Subject::Group(interns.id.get()), Permission::Deny),
    ];

    w.sharing.create(share("projects", rules), &w.admin.actor(), &request).await.unwrap();

    let seen = w.sharing.subject_access(Subject::Account(BOB)).await.unwrap();
    assert_eq!(seen[0].effective, None, "the database denies bob");
    let (folder, _) = last_acls(&w.host, "projects");
    assert_eq!(folder.as_deref(), Some("access 1 projects [@staff:rw,bob:-] -[]"));
}

/// bob's own write rule on `docs` is taken away (he still reads through
/// `readers`); the walk that strips his entries from the files fails. The
/// share stays owed and is walked again with the rules of now, until a
/// walk ends without errors; walks replace what is there (the files agent
/// takes away what the rules do not grant, whatever it was told to
/// remove), so bob's entries go however many walks failed before.
#[tokio::test]
async fn a_revocation_whose_walk_failed_is_made_up_later() {
    let w = world();
    let request = RequestContext::default();
    let actor = w.admin.actor();
    let readers = w
        .identity
        .create_group
        .execute(&w.admin, group("readers", vec![AccountId::new(BOB)]), &request)
        .await
        .unwrap();
    let base = vec![
        rule(Subject::Account(ALICE), Permission::ReadWrite),
        rule(Subject::Group(readers.id.get()), Permission::Read),
    ];
    let mut with_bob = base.clone();
    with_bob.push(rule(Subject::Account(BOB), Permission::ReadWrite));
    let docs = w.sharing.create(share("docs", with_bob), &actor, &request).await.unwrap();

    w.host.fail_walks.store(true, Ordering::Relaxed);
    let revoke = ShareChanges { access: Some(base), ..ShareChanges::default() };
    let (_, walk) = w.sharing.update(docs.id, revoke, &actor, &request).await.unwrap();
    assert_eq!(finished(&w.jobs, walk.unwrap().id).await, JobState::Failed);
    assert!(w.shares.owes(docs.id), "a walk with errors settles nothing");
    w.host.fail_walks.store(false, Ordering::Relaxed);
    assert!(w.sharing.reconcile_access(&request).await.unwrap().is_empty(), "not again at once");

    // A few minutes later zendiloosd looks again.
    w.clock.advance(Duration::from_secs(5 * 60));
    let again = w.sharing.reconcile_access(&request).await.unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(finished(&w.jobs, again[0].id).await, JobState::Succeeded);

    let calls = w.host.calls();
    let walks: Vec<&String> = calls.iter().filter(|c| c.ends_with(" recursive")).collect();
    assert_eq!(walks.first().map(|c| c.starts_with("failed ")), Some(true), "{walks:?}");
    assert_eq!(walks.last().map(|c| c.as_str()), Some("access 1 docs [alice:rw,@readers:r] -[] recursive"));
    assert!(!w.shares.owes(docs.id));
}

/// A rollback returns the share's dataset to the snapshot, its folder's
/// ACL and every file's ACL and owner included. bob's access, revoked
/// after the snapshot, is not back: the folder gets the rules recorded now
/// at once, under the same lock, and the files in a walk.
#[tokio::test]
async fn a_rollback_gives_the_rules_recorded_now_again() {
    let w = world();
    let request = RequestContext::default();
    let actor = w.admin.actor();
    let rules = vec![
        rule(Subject::Account(ALICE), Permission::ReadWrite),
        rule(Subject::Account(BOB), Permission::ReadWrite),
    ];
    let photos = w.sharing.create(share("photos", rules), &actor, &request).await.unwrap();
    let snapshot = w.sharing.take_snapshot(photos.id, "", &actor, &request).await.unwrap();
    let revoke = ShareChanges {
        access: Some(vec![rule(Subject::Account(ALICE), Permission::ReadWrite)]),
        ..ShareChanges::default()
    };
    let (_, walk) = w.sharing.update(photos.id, revoke, &actor, &request).await.unwrap();
    assert_eq!(finished(&w.jobs, walk.unwrap().id).await, JobState::Succeeded);
    let before = w.host.calls().len();

    let (_, walk) =
        w.sharing.rollback_snapshot(photos.id, &snapshot.label, "photos", &actor, &request).await.unwrap();
    assert_eq!(finished(&w.jobs, walk.unwrap().id).await, JobState::Succeeded);

    let after: Vec<String> = w.host.calls()[before..].to_vec();
    assert_eq!(
        after,
        [
            format!("rollback tank/photos@{}", snapshot.label),
            "access 1 photos [alice:rw] -[]".to_owned(),
            "access 1 photos [alice:rw] -[] recursive".to_owned(),
        ]
    );
    assert!(!w.shares.owes(photos.id));
}

/// bob, who may write `photos`, is disabled: Samba no longer lets him in,
/// and neither may his ID over NFS. The share's folder and files deny him
/// and what he owns passes to root, as for a denial; enabled again, his
/// rule holds again.
#[tokio::test]
async fn a_disabled_account_is_denied_on_the_shares_it_could_use() {
    let w = world();
    let request = RequestContext::default();
    let rules = vec![
        rule(Subject::Account(ALICE), Permission::ReadWrite),
        rule(Subject::Account(BOB), Permission::ReadWrite),
    ];
    let photos = w.sharing.create(share("photos", rules), &w.admin.actor(), &request).await.unwrap();
    w.sharing.create(share("media", vec![]), &w.admin.actor(), &request).await.unwrap();

    let disable = AccountChanges { disabled: Some(true), ..AccountChanges::default() };
    let (_, pending) =
        w.identity.update_account.execute(&w.admin, AccountId::new(BOB), disable, &request).await.unwrap();
    assert_eq!(pending, ["photos"], "media never named him");
    all_finished(&w.jobs).await;
    let (folder, tree) = last_acls(&w.host, "photos");
    assert_eq!(folder.as_deref(), Some("access 1 photos [alice:rw,bob:-] -[] recursive"));
    assert_eq!(tree, folder);

    let enable = AccountChanges { disabled: Some(false), ..AccountChanges::default() };
    w.identity.update_account.execute(&w.admin, AccountId::new(BOB), enable, &request).await.unwrap();
    all_finished(&w.jobs).await;
    let (_, tree) = last_acls(&w.host, "photos");
    assert_eq!(tree.as_deref(), Some("access 1 photos [alice:rw,bob:rw] -[] recursive"));
    assert!(!w.shares.owes(photos.id));
}

/// bob is deleted: his ID stays in the ACLs and owns his files, which an
/// NFS client still claiming it would reach. His entries go and his files
/// pass to root.
#[tokio::test]
async fn a_deleted_account_leaves_nothing_its_id_could_use() {
    let w = world();
    let request = RequestContext::default();
    let rules = vec![
        rule(Subject::Account(ALICE), Permission::ReadWrite),
        rule(Subject::Account(BOB), Permission::ReadWrite),
    ];
    let photos = w.sharing.create(share("photos", rules), &w.admin.actor(), &request).await.unwrap();
    let before = w.host.calls().len();

    w.identity.delete_account.execute(&w.admin, AccountId::new(BOB), &request).await.unwrap();
    all_finished(&w.jobs).await;

    assert_eq!(
        w.host.calls()[before..],
        ["access 1 photos [alice:rw] -[]", "access 1 photos [alice:rw] -[] recursive"]
    );
    assert!(!w.shares.owes(photos.id));
}
