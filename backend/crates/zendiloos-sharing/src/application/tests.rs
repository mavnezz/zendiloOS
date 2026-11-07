use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
use zendiloos_jobs::{Job, JobState, Jobs};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, PortError, RequestContext};
use zendiloos_notify::Notifier;
use zendiloos_notify::testing::MemoryNotifications;

use super::ports::{Directory, Names};
use super::{
    FilesError, FilesUseCases, NewShareInput, ShareChanges, SharingError, SharingPorts, SharingUseCases,
    Transfer,
};
use crate::domain::{
    Access, AccessRule, Favourite, FilePath, FileServices, Permission, Principal, ShareId, SmbProtocol,
    Subject,
};
use crate::testing::{Files, Host, Shares, Volumes};

struct Static;

#[async_trait]
impl Directory for Static {
    async fn names(&self) -> Result<Names, PortError> {
        let mut names = Names::default();
        names.accounts.insert(1, "alice".into());
        names.accounts.insert(2, "bob".into());
        names.groups.insert(10, "family".into());
        names.members.insert(10, vec![1]);
        Ok(names)
    }

    /// alice (1) is in family (10).
    async fn memberships(&self, account: i64) -> Result<Vec<i64>, PortError> {
        Ok(if account == 1 { vec![10] } else { vec![] })
    }
}

struct NoAudit;

#[async_trait]
impl AuditLog for NoAudit {
    async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
        Ok(())
    }
}

/// A clock tests move by hand; it starts at 2026-11-05 00:00 UTC.
#[derive(Default)]
struct Moving(std::sync::atomic::AtomicI64);

impl Moving {
    const START: i64 = 1_793_836_800;

    fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, std::sync::atomic::Ordering::Relaxed);
    }
}

impl zendiloos_kernel::Clock for Moving {
    fn now(&self) -> zendiloos_kernel::Timestamp {
        zendiloos_kernel::Timestamp::from_unix_seconds(
            Self::START + self.0.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

struct Setup {
    sharing: SharingUseCases,
    files: FilesUseCases,
    host: Arc<Host>,
    file_host: Arc<Files>,
    jobs: Jobs,
    clock: Arc<Moving>,
    notices: Arc<MemoryNotifications>,
}

fn setup() -> Setup {
    let host = Arc::new(Host::default());
    let file_host = Arc::new(Files::default());
    let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
    let clock = Arc::new(Moving::default());
    let notices = Arc::new(MemoryNotifications::default());
    let ports = SharingPorts {
        shares: Arc::new(Shares::default()),
        directory: Arc::new(Static),
        volumes: Arc::new(Volumes),
        host: host.clone(),
        files: file_host.clone(),
        jobs: jobs.clone(),
        audit: Arc::new(NoAudit),
        clock: clock.clone(),
        notifier: Notifier::new(notices.clone(), Arc::new(FixedClock)),
    };
    Setup {
        sharing: SharingUseCases::new(ports.clone()),
        files: FilesUseCases::new(ports),
        host,
        file_host,
        jobs,
        clock,
        notices,
    }
}

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn rule(subject: Subject, access: Access) -> AccessRule {
    AccessRule { subject, permission: access.into() }
}

fn deny(subject: Subject) -> AccessRule {
    AccessRule { subject, permission: Permission::Deny }
}

fn input(name: &str, pool: &str) -> NewShareInput {
    NewShareInput {
        name: name.into(),
        description: Some(" Holiday photos ".into()),
        pool: pool.into(),
        recycle_bin: true,
        access: vec![rule(Subject::Group(10), Access::ReadWrite), rule(Subject::Account(2), Access::Read)],
        nfs: vec![("192.168.1.0/24".into(), false)],
        settings: crate::domain::ShareSettings::default(),
    }
}

async fn finished(jobs: &Jobs, job: Job) -> Job {
    for _ in 0..100 {
        let current = jobs.find(job.id).await.unwrap().unwrap();
        if current.state != JobState::Running {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("job did not finish");
}

#[tokio::test]
async fn creating_a_share_makes_its_dataset_permissions_and_configuration() {
    let s = setup();
    let share =
        s.sharing.create(input("photos", "tank"), &alice(), &RequestContext::default()).await.unwrap();

    assert_eq!(share.path(), "/volume1/photos");
    assert_eq!(share.description.as_deref(), Some("Holiday photos"));
    assert_eq!(share.nfs[0].network, "192.168.1.0/24");
    assert_eq!(
        s.host.calls(),
        [
            "create tank 1 photos",
            "properties tank/photos refquota=none compression=lz4 recordsize=131072",
            "access 1 photos [@family:rw,bob:r] -[]",
            "apply 1"
        ]
    );
    let config = s.host.configs.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        config.smb[0].access,
        [
            (Principal::Group("family".into()), Permission::ReadWrite),
            (Principal::User("bob".into()), Permission::Read)
        ]
    );
    assert_eq!(config.nfs.len(), 1);
}

#[tokio::test]
async fn bad_requests_change_nothing() {
    let s = setup();
    let (request, actor) = (RequestContext::default(), alice());
    let create = |input| s.sharing.create(input, &actor, &request);

    assert!(matches!(create(input("homes", "tank")).await, Err(SharingError::InvalidName(_))));
    assert!(
        matches!(create(input("photos", "cold")).await, Err(SharingError::VolumeNotFound)),
        "not mounted"
    );
    assert!(matches!(create(input("photos", "nope")).await, Err(SharingError::VolumeNotFound)));
    let mut unknown = input("photos", "tank");
    unknown.access.push(rule(Subject::Account(99), Access::Read));
    assert!(matches!(create(unknown).await, Err(SharingError::UnknownSubject)));
    let mut twice = input("photos", "tank");
    twice.access.push(rule(Subject::Group(10), Access::Read));
    assert!(matches!(create(twice).await, Err(SharingError::Duplicate)));
    let mut network = input("photos", "tank");
    network.nfs = vec![("*".into(), true)];
    assert!(matches!(create(network).await, Err(SharingError::InvalidNetwork(_))));
    assert_eq!(s.host.calls(), Vec::<String>::new());
}

#[tokio::test]
async fn a_failed_creation_removes_the_dataset() {
    let s = setup();
    s.host.refuse_access.store(true, std::sync::atomic::Ordering::Relaxed);
    let result = s.sharing.create(input("photos", "tank"), &alice(), &RequestContext::default()).await;
    assert!(matches!(result, Err(SharingError::Refused(_))), "{result:?}");
    assert_eq!(
        s.host.calls(),
        [
            "create tank 1 photos",
            "properties tank/photos refquota=none compression=lz4 recordsize=131072",
            "destroy tank 1 photos"
        ]
    );
    assert!(s.sharing.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_share_whose_services_cannot_apply_leaves_neither_record_nor_dataset() {
    let s = setup();
    s.host.refuse_apply.store(true, std::sync::atomic::Ordering::Relaxed);

    let result = s.sharing.create(input("photos", "tank"), &alice(), &RequestContext::default()).await;

    assert!(result.is_err(), "{result:?}");
    assert!(s.sharing.list().await.unwrap().is_empty(), "the record went");
    assert_eq!(s.host.calls().last().unwrap(), "destroy tank 1 photos");
    s.host.refuse_apply.store(false, std::sync::atomic::Ordering::Relaxed);
    let again = s.sharing.create(input("photos", "tank"), &alice(), &RequestContext::default()).await;
    assert!(again.is_ok(), "the name is free again: {again:?}");
}

#[tokio::test]
async fn changed_access_applies_now_and_to_existing_files_in_a_job() {
    let s = setup();
    let request = RequestContext::default();
    let share = s.sharing.create(input("photos", "tank"), &alice(), &request).await.unwrap();

    let changes = ShareChanges {
        access: Some(vec![
            rule(Subject::Account(1), Access::ReadWrite),
            rule(Subject::Group(10), Access::Read),
        ]),
        ..ShareChanges::default()
    };
    let (updated, job) = s.sharing.update(share.id, changes, &alice(), &request).await.unwrap();
    assert_eq!(updated.access.len(), 2);
    assert_eq!(finished(&s.jobs, job.unwrap()).await.state, JobState::Succeeded);
    assert_eq!(
        &s.host.calls()[4..],
        [
            "access 1 photos [alice:rw,@family:r] -[bob]",
            "apply 1",
            "access 1 photos [alice:rw,@family:r] -[bob] recursive",
        ]
    );

    let (_, none) = s
        .sharing
        .update(
            share.id,
            ShareChanges { recycle_bin: Some(false), ..ShareChanges::default() },
            &alice(),
            &request,
        )
        .await
        .unwrap();
    assert!(none.is_none(), "no job without new access rules");
}

#[tokio::test]
async fn deleting_needs_the_name_and_stops_offering_first() {
    let s = setup();
    let request = RequestContext::default();
    let share = s.sharing.create(input("photos", "tank"), &alice(), &request).await.unwrap();

    let wrong = s.sharing.delete(share.id, "fotos", &alice(), &request).await;
    assert!(matches!(wrong, Err(SharingError::ConfirmationMismatch)));
    let job = s.sharing.delete(share.id, "photos", &alice(), &request).await.unwrap();
    assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
    assert_eq!(&s.host.calls()[4..], ["apply 0", "destroy tank 1 photos"]);
    assert!(s.sharing.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_deletion_that_cannot_apply_leaves_the_share_as_it_was() {
    let s = setup();
    let request = RequestContext::default();
    let share = s.sharing.create(input("photos", "tank"), &alice(), &request).await.unwrap();
    s.host.refuse_apply.store(true, std::sync::atomic::Ordering::Relaxed);

    let refused = s.sharing.delete(share.id, "photos", &alice(), &request).await;

    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(s.sharing.list().await.unwrap().len(), 1, "still there, to be deleted again");
    assert!(!s.host.calls().iter().any(|c| c.starts_with("destroy")));
}

#[tokio::test]
async fn data_that_cannot_go_now_goes_when_zendiloos_starts_again() {
    let s = setup();
    let request = RequestContext::default();
    let share = s.sharing.create(input("photos", "tank"), &alice(), &request).await.unwrap();
    s.host.refuse_destroy.store(true, std::sync::atomic::Ordering::Relaxed);

    let job = s.sharing.delete(share.id, "photos", &alice(), &request).await.unwrap();
    assert_eq!(finished(&s.jobs, job).await.state, JobState::Failed);

    assert!(s.sharing.list().await.unwrap().is_empty(), "offered no more");
    let taken = s.sharing.create(input("photos", "tank"), &alice(), &request).await;
    assert!(matches!(taken, Err(SharingError::NameTaken)), "the name waits for the data: {taken:?}");
    assert_eq!(s.sharing.finish_deletions(&request).await.unwrap(), 1, "still busy");
    s.host.refuse_destroy.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(s.sharing.finish_deletions(&request).await.unwrap(), 0);
    assert!(s.sharing.create(input("photos", "tank"), &alice(), &request).await.is_ok(), "free again");
}

#[tokio::test]
async fn apps_get_access_beside_the_people_and_keep_it_when_rules_change() {
    let s = setup();
    let request = RequestContext::default();
    let share = s.sharing.create(input("photos", "tank"), &alice(), &request).await.unwrap();
    let photos = share.name.clone();

    s.sharing.set_app_access("app-immich", &[(photos.clone(), Access::ReadWrite)], &request).await.unwrap();
    assert_eq!(
        &s.host.calls()[4..],
        [
            "access 1 photos [@family:rw,bob:r,app-immich:rw] -[]",
            "access 1 photos [@family:rw,bob:r,app-immich:rw] -[] recursive",
        ]
    );

    // The people's rules change; the app's entry stays.
    let changes = ShareChanges {
        access: Some(vec![rule(Subject::Account(1), Access::ReadWrite)]),
        ..ShareChanges::default()
    };
    let (_, job) = s.sharing.update(share.id, changes, &alice(), &request).await.unwrap();
    finished(&s.jobs, job.unwrap()).await;
    assert!(s.host.calls().contains(&"access 1 photos [alice:rw,app-immich:rw] -[@family,bob]".to_owned()));
    // While the app mounts it, the share stays.
    let deleted = s.sharing.delete(share.id, "photos", &alice(), &request).await;
    assert!(matches!(deleted, Err(SharingError::UsedByApps(ref apps)) if apps == "immich"), "{deleted:?}");

    // Uninstalled: the entries go, and the share can go too.
    s.sharing.set_app_access("app-immich", &[], &request).await.unwrap();
    assert!(s.host.calls().contains(&"access 1 photos [alice:rw] -[app-immich] recursive".to_owned()));
    let job = s.sharing.delete(share.id, "photos", &alice(), &request).await.unwrap();
    assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);

    let unknown = s.sharing.set_app_access("app-immich", &[(photos, Access::Read)], &request).await;
    assert!(matches!(unknown, Err(SharingError::ShareNotFound)));
}

fn bob() -> Actor {
    Actor { account_id: 2, name: "bob".into() }
}

fn carol() -> Actor {
    Actor { account_id: 3, name: "carol".into() }
}

fn file(path: &str) -> FilePath {
    FilePath::parse(path).unwrap()
}

/// `photos`: family (alice) may write, bob may read.
async fn with_photos(s: &Setup) -> ShareId {
    s.sharing.create(input("photos", "tank"), &alice(), &RequestContext::default()).await.unwrap().id
}

#[tokio::test]
async fn users_see_the_shares_their_rules_name() {
    let s = setup();
    let id = with_photos(&s).await;
    let request = RequestContext::default();

    let alices = s.files.shares(&alice()).await.unwrap();
    assert_eq!((alices[0].share.id, alices[0].access), (id, Access::ReadWrite), "through the group");
    assert_eq!(s.files.shares(&bob()).await.unwrap()[0].access, Access::Read);
    assert!(s.files.shares(&carol()).await.unwrap().is_empty());

    let listing = s.files.list(&bob(), id, file("2024"), None, &request).await.unwrap();
    assert_eq!(listing.entries.len(), 1);
    assert!(matches!(
        s.files.list(&carol(), id, FilePath::root(), None, &request).await,
        Err(FilesError::ShareNotFound)
    ));
    assert!(matches!(
        s.files.list(&bob(), ShareId::new(99), FilePath::root(), None, &request).await,
        Err(FilesError::ShareNotFound)
    ));
    assert!(matches!(
        s.files.open(&bob(), id, file("missing.jpg"), None, &request).await,
        Err(FilesError::NotFound)
    ));
    let (_, size, name) = s.files.archive(&bob(), id, FilePath::root(), None, &request).await.unwrap();
    assert_eq!((size, name.as_str()), (10, "photos"), "the share's top is named after it");
    let (_, _, name) = s.files.archive(&bob(), id, file("2024"), None, &request).await.unwrap();
    assert_eq!(name, "2024");
    assert_eq!(
        s.file_host.calls(),
        [
            "list bob photos:2024",
            "open bob photos:missing.jpg",
            "archive bob photos:",
            "archive bob photos:2024"
        ],
        "as the user"
    );
}

#[tokio::test]
async fn readers_cannot_change_anything() {
    let s = setup();
    let id = with_photos(&s).await;
    let (request, bob) = (RequestContext::default(), bob());

    let upload = s.files.begin_upload(&bob, id, FilePath::root(), "a.jpg", false, &request).await;
    assert!(matches!(upload, Err(FilesError::Denied)));
    assert!(matches!(s.files.make_directory(&bob, id, file("new"), &request).await, Err(FilesError::Denied)));
    assert!(matches!(
        s.files.rename(&bob, id, file("a.jpg"), "b.jpg", &request).await,
        Err(FilesError::Denied)
    ));
    assert!(matches!(s.files.delete(&bob, id, vec![file("a.jpg")], &request).await, Err(FilesError::Denied)));
    let restore = s.files.restore(&bob, id, vec![file(".recycle/bob/a.jpg")], &request).await;
    assert!(matches!(restore, Err(FilesError::Denied)));
    let copy = s.files.transfer(&bob, Transfer::Copy, id, vec![file("a.jpg")], id, file("x"), &request).await;
    assert!(matches!(copy, Err(FilesError::Denied)), "the target is not writable");
    assert_eq!(s.file_host.calls(), Vec::<String>::new());
}

#[tokio::test]
async fn uploads_get_their_name_at_the_end() {
    let s = setup();
    let id = with_photos(&s).await;
    let (request, alice) = (RequestContext::default(), alice());

    let upload = s.files.begin_upload(&alice, id, file("2024"), "a.jpg", false, &request).await.unwrap();
    let path = s.files.finish_upload(&alice, upload.ticket, &request).await.unwrap();
    assert_eq!(path, file("2024/a.jpg"));

    let upload = s.files.begin_upload(&alice, id, file("2024"), "taken.jpg", false, &request).await.unwrap();
    assert!(matches!(s.files.finish_upload(&alice, upload.ticket, &request).await, Err(FilesError::Exists)));
    assert_eq!(
        s.file_host.calls(),
        [
            "create_upload alice photos:2024",
            "move alice photos:2024/.zendiloos-upload-1 -> photos:2024/a.jpg",
            "create_upload alice photos:2024",
            "move alice photos:2024/.zendiloos-upload-1 -> photos:2024/taken.jpg",
            "remove alice photos:2024/.zendiloos-upload-1",
        ]
    );
    for name in ["", "..", "a/b", ".zendiloos-upload-2"] {
        let upload = s.files.begin_upload(&alice, id, FilePath::root(), name, false, &request).await;
        assert!(matches!(upload, Err(FilesError::InvalidName)), "{name:?}");
    }
}

#[tokio::test]
async fn copies_moves_and_deletes_run_as_jobs() {
    let s = setup();
    let id = with_photos(&s).await;
    let (request, alice) = (RequestContext::default(), alice());

    let into_itself = s
        .files
        .transfer(&alice, Transfer::Move, id, vec![file("album")], id, file("album/deep"), &request)
        .await;
    assert!(matches!(into_itself, Err(FilesError::Invalid(_))));

    let renamed = s.files.rename(&alice, id, file("album/a.jpg"), "b.jpg", &request).await.unwrap();
    assert_eq!(renamed, file("album/b.jpg"));
    let copy = s
        .files
        .transfer(
            &alice,
            Transfer::Copy,
            id,
            vec![file("album"), file("b.jpg")],
            id,
            file("backup"),
            &request,
        )
        .await
        .unwrap();
    assert_eq!((copy.kind.as_str(), copy.target.as_deref()), ("files.copy", Some("photos")));
    assert_eq!(finished(&s.jobs, copy).await.state, JobState::Succeeded);
    let delete = s.files.delete(&alice, id, vec![file("b.jpg")], &request).await.unwrap();
    assert_eq!(finished(&s.jobs, delete).await.state, JobState::Succeeded);
    let failing = s.files.delete(&alice, id, vec![file("missing")], &request).await.unwrap();
    let failed = finished(&s.jobs, failing).await;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.error.map(|e| e.code).as_deref(), Some("not_found"));

    assert_eq!(
        s.file_host.calls(),
        [
            "move alice photos:album/a.jpg -> photos:album/b.jpg",
            "copy alice photos:album -> photos:backup/album",
            "copy alice photos:b.jpg -> photos:backup/b.jpg",
            "remove alice photos:b.jpg recycle",
            "remove alice photos:missing recycle",
        ]
    );
}

#[tokio::test]
async fn favourites_are_per_account_and_only_in_usable_shares() {
    let s = setup();
    let id = with_photos(&s).await;
    let favourite = |path: &str| Favourite { share: id, path: file(path) };

    let saved = s.files.set_favourites(&bob(), vec![favourite("2024"), favourite("")]).await.unwrap();
    assert_eq!(saved.iter().map(|(f, _)| f.path.as_str()).collect::<Vec<_>>(), ["2024", ""]);
    assert_eq!(saved[0].1.as_str(), "photos");
    assert!(s.files.favourites(&alice()).await.unwrap().is_empty(), "per account");
    let elsewhere = s.files.set_favourites(&carol(), vec![favourite("2024")]).await;
    assert!(matches!(elsewhere, Err(FilesError::ShareNotFound)), "carol may not use the share");
    let twice = s.files.set_favourites(&bob(), vec![favourite("2024"), favourite("2024")]).await;
    assert!(matches!(twice, Err(FilesError::Invalid(_))));
}

#[tokio::test]
async fn snapshots_are_looked_into_and_restored_from_without_replacing() {
    let s = setup();
    let id = with_photos(&s).await;
    let (request, label) = (RequestContext::default(), "zdl-2026-11-05_19.40.01");
    s.host.snapshots.lock().unwrap().push(crate::domain::Snapshot {
        pool: "tank".into(),
        share: crate::domain::ShareName::parse("photos").unwrap(),
        label: label.into(),
        taken_at: zendiloos_kernel::Timestamp::from_unix_seconds(1_793_907_601),
        schedules: vec![],
        note: Some("before the move".into()),
        used_bytes: 0,
        referenced_bytes: 0,
    });

    let seen = s.files.snapshots(&bob(), id, &request).await.unwrap();
    assert_eq!(seen.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), [label]);
    assert!(matches!(s.files.snapshots(&carol(), id, &request).await, Err(FilesError::ShareNotFound)));
    s.files.list(&bob(), id, file("2024"), Some(label), &request).await.unwrap();
    let odd = s.files.list(&bob(), id, FilePath::root(), Some("../.."), &request).await;
    assert!(matches!(odd, Err(FilesError::Invalid(_))), "{odd:?}");

    let reader = s.files.restore_from_snapshot(&bob(), id, label, vec![file("a.jpg")], &request).await;
    assert!(matches!(reader, Err(FilesError::Denied)), "readers cannot write back");
    let job = s
        .files
        .restore_from_snapshot(&alice(), id, label, vec![file("album/a.jpg")], &request)
        .await
        .unwrap();
    assert_eq!(job.kind.as_str(), "files.restore_snapshot");
    assert_eq!(finished(&s.jobs, job).await.state, JobState::Succeeded);
    assert_eq!(
        s.file_host.calls(),
        [
            format!("list bob photos@{label}:2024"),
            format!("copy alice photos@{label}:album/a.jpg -> photos:album/a.jpg"),
        ]
    );
}

#[tokio::test]
async fn restoring_takes_entries_out_of_the_bin_one_by_one() {
    let s = setup();
    let id = with_photos(&s).await;
    let (request, alice) = (RequestContext::default(), alice());

    let restored = s
        .files
        .restore(&alice, id, vec![file(".recycle/alice/album/a.jpg"), file(".recycle/alice/b.jpg")], &request)
        .await
        .unwrap();
    assert_eq!(restored, [file("album/a.jpg"), file("b.jpg")]);
    let gone = s.files.restore(&alice, id, vec![file(".recycle/alice/missing")], &request).await;
    assert!(matches!(gone, Err(FilesError::NotFound)), "{gone:?}");
    assert!(matches!(s.files.restore(&alice, id, vec![], &request).await, Err(FilesError::Invalid(_))));
    assert_eq!(
        s.file_host.calls(),
        [
            "restore alice photos:.recycle/alice/album/a.jpg",
            "restore alice photos:.recycle/alice/b.jpg",
            "restore alice photos:.recycle/alice/missing",
        ]
    );
}

// Snapshots

async fn photos(s: &Setup) -> ShareId {
    let input = NewShareInput {
        name: "photos".into(),
        description: None,
        pool: "tank".into(),
        recycle_bin: true,
        access: vec![rule(Subject::Account(1), Access::ReadWrite)],
        nfs: vec![],
        settings: crate::domain::ShareSettings::default(),
    };
    s.sharing.create(input, &alice(), &RequestContext::default()).await.unwrap().id
}

#[tokio::test]
async fn snapshots_by_hand_are_listed_and_deleted_one_by_one() {
    let s = setup();
    let request = RequestContext::default();
    let id = photos(&s).await;

    let taken = s.sharing.take_snapshot(id, " before the move ", &alice(), &request).await.unwrap();
    assert_eq!(taken.label, "zdl-2026-11-05_00.00.00");
    assert_eq!(taken.note.as_deref(), Some("before the move"));
    let again = s.sharing.take_snapshot(id, "", &alice(), &request).await;
    assert!(matches!(again, Err(SharingError::SnapshotTaken)), "same second: {again:?}");
    let bad = s.sharing.take_snapshot(id, "two\nlines", &alice(), &request).await;
    assert!(matches!(bad, Err(SharingError::InvalidNote(_))), "{bad:?}");

    let (listed, schedules) = s.sharing.snapshots(id, &request).await.unwrap();
    assert_eq!((listed.len(), schedules.len()), (1, 0));
    let missing = s.sharing.delete_snapshot(id, "zdl-2026-01-01_00.00.00", &alice(), &request).await;
    assert!(matches!(missing, Err(SharingError::SnapshotNotFound)), "{missing:?}");
    let not_a_label = s.sharing.delete_snapshot(id, "-r", &alice(), &request).await;
    assert!(matches!(not_a_label, Err(SharingError::SnapshotNotFound)), "{not_a_label:?}");
    s.sharing.delete_snapshot(id, &taken.label, &alice(), &request).await.unwrap();
    assert!(s.sharing.snapshots(id, &request).await.unwrap().0.is_empty());
}

#[tokio::test]
async fn rolling_back_needs_the_name_and_lets_go_of_later_snapshots() {
    let s = setup();
    let request = RequestContext::default();
    let id = photos(&s).await;
    let first = s.sharing.take_snapshot(id, "", &alice(), &request).await.unwrap();
    s.clock.advance(3600);
    let second = s.sharing.take_snapshot(id, "", &alice(), &request).await.unwrap();

    let unconfirmed = s.sharing.rollback_snapshot(id, &first.label, "photo", &alice(), &request).await;
    assert!(matches!(unconfirmed, Err(SharingError::ConfirmationMismatch)), "{unconfirmed:?}");
    let missing =
        s.sharing.rollback_snapshot(id, "zdl-2020-01-01_00.00.00", "photos", &alice(), &request).await;
    assert!(matches!(missing, Err(SharingError::SnapshotNotFound)), "{missing:?}");

    let gone = s.sharing.rollback_snapshot(id, &first.label, "photos", &alice(), &request).await.unwrap();
    assert_eq!(gone, std::slice::from_ref(&second.label));
    let (left, _) = s.sharing.snapshots(id, &request).await.unwrap();
    assert_eq!(left.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), [first.label.as_str()]);
    assert!(s.host.calls().contains(&format!("rollback tank/photos@{}", first.label)));
}

#[tokio::test]
async fn schedules_take_snapshots_when_due_and_let_go_of_their_own_old_ones() {
    use crate::domain::{Frequency, SnapshotSchedule};
    let s = setup();
    let request = RequestContext::default();
    let id = photos(&s).await;
    let plan = |frequency, keep| SnapshotSchedule { frequency, keep };
    let invalid =
        s.sharing.set_snapshot_schedules(id, vec![plan(Frequency::Daily, 0)], &alice(), &request).await;
    assert!(matches!(invalid, Err(SharingError::InvalidSchedule(_))), "{invalid:?}");
    let schedules = vec![plan(Frequency::Daily, 1), plan(Frequency::Hourly, 2)];
    s.sharing.set_snapshot_schedules(id, schedules, &alice(), &request).await.unwrap();

    for _ in 0..4 {
        s.sharing.take_due_snapshots().await;
        s.clock.advance(3600);
    }

    let calls: Vec<String> = s.host.calls().into_iter().filter(|c| c.contains('@')).collect();
    assert_eq!(
        calls,
        [
            "snapshot tank/photos@zdl-2026-11-05_00.00.00 [hourly,daily]",
            "snapshot tank/photos@zdl-2026-11-05_01.00.00 [hourly]",
            "snapshot tank/photos@zdl-2026-11-05_02.00.00 [hourly]",
            "snapshot tank/photos@zdl-2026-11-05_03.00.00 [hourly]",
            // The hourly one of 01:00 is beyond the two kept; the first is
            // still the daily one.
            "destroy tank/photos@zdl-2026-11-05_01.00.00",
        ]
    );
}

#[tokio::test]
async fn a_failing_schedule_is_reported_once_until_it_works_again() {
    use crate::domain::{Frequency, SnapshotSchedule};
    let s = setup();
    let request = RequestContext::default();
    let id = photos(&s).await;
    let hourly = vec![SnapshotSchedule { frequency: Frequency::Hourly, keep: 3 }];
    s.sharing.set_snapshot_schedules(id, hourly, &alice(), &request).await.unwrap();
    s.host.refuse_snapshots.store(true, std::sync::atomic::Ordering::Relaxed);

    s.sharing.take_due_snapshots().await;
    s.clock.advance(300);
    s.sharing.take_due_snapshots().await;
    assert_eq!(s.notices.all().iter().map(|n| n.code.as_str()).collect::<Vec<_>>(), ["snapshot_failed"]);

    s.host.refuse_snapshots.store(false, std::sync::atomic::Ordering::Relaxed);
    s.sharing.take_due_snapshots().await;
    s.host.refuse_snapshots.store(true, std::sync::atomic::Ordering::Relaxed);
    s.clock.advance(3600);
    s.sharing.take_due_snapshots().await;
    assert_eq!(s.notices.all().len(), 2, "reported again after it had worked");
}

// Settings

#[tokio::test]
async fn settings_go_to_the_dataset_and_a_quota_never_below_what_is_there() {
    use crate::domain::{Compression, MIN_QUOTA_BYTES, RecordSize, ShareSettings};
    let s = setup();
    let request = RequestContext::default();
    let mut new = input("photos", "tank");
    new.settings = ShareSettings {
        quota_bytes: Some(2 * MIN_QUOTA_BYTES),
        compression: Compression::Zstd,
        record_size: RecordSize::M1,
    };
    let share = s.sharing.create(new, &alice(), &request).await.unwrap();
    assert!(s.host.calls().iter().any(|c| c
        == &format!(
            "properties tank/photos refquota={} compression=zstd recordsize=1048576",
            2 * MIN_QUOTA_BYTES
        )));
    let mut tiny = input("tiny", "tank");
    tiny.settings.quota_bytes = Some(1024);
    assert!(matches!(
        s.sharing.create(tiny, &alice(), &request).await,
        Err(SharingError::InvalidSettings(_))
    ));

    s.host.used_bytes.store(3 * MIN_QUOTA_BYTES, std::sync::atomic::Ordering::Relaxed);
    let smaller = ShareSettings { quota_bytes: Some(2 * MIN_QUOTA_BYTES), ..ShareSettings::default() };
    let refused = s
        .sharing
        .update(
            share.id,
            ShareChanges { settings: Some(smaller), ..ShareChanges::default() },
            &alice(),
            &request,
        )
        .await;
    assert!(matches!(refused, Err(SharingError::InvalidSettings(_))), "{refused:?}");
    let none = ShareSettings::default();
    let (updated, _) = s
        .sharing
        .update(
            share.id,
            ShareChanges { settings: Some(none), ..ShareChanges::default() },
            &alice(),
            &request,
        )
        .await
        .unwrap();
    assert_eq!(updated.settings, none);
    assert_eq!(s.sharing.usage(&request).await.unwrap()[0].1.used_bytes, 3 * MIN_QUOTA_BYTES);
}

#[tokio::test]
async fn file_services_are_applied_with_the_shares() {
    let s = setup();
    let request = RequestContext::default();
    assert_eq!(s.sharing.file_services().await.unwrap(), FileServices::default());
    s.sharing.create(input("photos", "tank"), &alice(), &request).await.unwrap();

    let wanted = FileServices {
        smb: true,
        nfs: false,
        workgroup: " home ".into(),
        smb_min_protocol: SmbProtocol::Smb3,
        ..FileServices::default()
    };
    let stored = s.sharing.set_file_services(wanted, &alice(), &request).await.unwrap();

    assert_eq!(stored.workgroup, "HOME");
    let config = s.host.configs.lock().unwrap().last().unwrap().clone();
    assert_eq!((config.smb.len(), config.services), (1, stored.clone()));
    assert_eq!(s.sharing.file_services().await.unwrap(), stored);
}

#[tokio::test]
async fn file_services_the_host_refuses_give_way_to_the_previous_ones() {
    let s = setup();
    let request = RequestContext::default();
    s.host.refuse_all_off.store(true, std::sync::atomic::Ordering::Relaxed);
    let off = FileServices { smb: false, nfs: false, ..FileServices::default() };

    let result = s.sharing.set_file_services(off, &alice(), &request).await;

    assert!(matches!(result, Err(SharingError::Refused(_))), "{result:?}");
    assert_eq!(s.sharing.file_services().await.unwrap(), FileServices::default());
    assert_eq!(s.host.configs.lock().unwrap().last().unwrap().services, FileServices::default());

    let bad = FileServices { workgroup: "A B".into(), ..FileServices::default() };
    let result = s.sharing.set_file_services(bad, &alice(), &request).await;
    assert!(matches!(result, Err(SharingError::InvalidServices(_))));
}

/// photos with alice (1) writing and nobody else.
async fn alices_photos(s: &Setup) -> ShareId {
    let mut photos = input("photos", "tank");
    photos.access = vec![rule(Subject::Account(1), Access::ReadWrite)];
    photos.nfs = vec![];
    s.sharing.create(photos, &alice(), &RequestContext::default()).await.unwrap().id
}

#[tokio::test]
async fn a_denied_group_overrides_its_members_own_rules() {
    let s = setup();
    let request = RequestContext::default();
    let photos = alices_photos(&s).await;

    let jobs = s
        .sharing
        .set_subject_access(Subject::Group(10), &[(photos, Some(Permission::Deny))], &alice(), &request)
        .await
        .unwrap();

    assert_eq!(jobs.len(), 1);
    finished(&s.jobs, jobs[0].clone()).await;
    let calls = s.host.calls();
    assert!(calls.contains(&"access 1 photos [alice:-] -[]".to_owned()), "alice is in family: {calls:?}");
    assert!(calls.contains(&"access 1 photos [alice:-] -[] recursive".to_owned()), "{calls:?}");
    let config = s.host.configs.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        config.smb[0].access,
        [
            (Principal::User("alice".into()), Permission::ReadWrite),
            (Principal::Group("family".into()), Permission::Deny)
        ],
        "Samba gets the group: it follows its members itself"
    );
    let seen = s.sharing.subject_access(Subject::Account(1)).await.unwrap();
    assert_eq!((seen[0].rule, seen[0].effective), (Some(Permission::ReadWrite), None));
    let group = s.sharing.subject_access(Subject::Group(10)).await.unwrap();
    assert_eq!((group[0].rule, group[0].effective), (Some(Permission::Deny), None));
    assert!(matches!(s.sharing.subject_access(Subject::Group(99)).await, Err(SharingError::UnknownSubject)));
}

#[tokio::test]
async fn members_who_leave_a_denied_group_lose_their_entry() {
    let s = setup();
    let request = RequestContext::default();
    let photos = alices_photos(&s).await;
    s.sharing
        .set_subject_access(Subject::Group(10), &[(photos, Some(Permission::Deny))], &alice(), &request)
        .await
        .unwrap();

    // bob (2) was in family and has left; alice (1) still is.
    let jobs = s.sharing.group_members_changed(10, &[1, 2], &alice(), &request).await.unwrap();

    assert_eq!(jobs.len(), 1);
    finished(&s.jobs, jobs[0].clone()).await;
    let calls = s.host.calls();
    assert!(calls.contains(&"access 1 photos [alice:-] -[bob] recursive".to_owned()), "{calls:?}");
    assert!(
        s.sharing.group_members_changed(77, &[1], &alice(), &request).await.unwrap().is_empty(),
        "denied nowhere"
    );
}

#[tokio::test]
async fn a_group_about_to_go_takes_its_rules_and_its_members_entries_along() {
    let s = setup();
    let request = RequestContext::default();
    let photos = alices_photos(&s).await;
    s.sharing
        .set_subject_access(Subject::Group(10), &[(photos, Some(Permission::Deny))], &alice(), &request)
        .await
        .unwrap();

    let jobs = s.sharing.group_deleting(10, &alice(), &request).await.unwrap();

    finished(&s.jobs, jobs[0].clone()).await;
    let calls = s.host.calls();
    assert!(
        calls.contains(&"access 1 photos [alice:rw] -[] recursive".to_owned()),
        "alice's own rule is back: {calls:?}"
    );
    let share = s.sharing.list().await.unwrap().into_iter().find(|share| share.id == photos).unwrap();
    assert_eq!(share.rule_of(Subject::Group(10)), None);
}

#[tokio::test]
async fn subject_rules_are_checked_before_anything_changes() {
    let s = setup();
    let request = RequestContext::default();
    let photos = alices_photos(&s).await;
    let calls = s.host.calls().len();

    let twice = [(photos, Some(Permission::Read)), (photos, None)];
    assert!(matches!(
        s.sharing.set_subject_access(Subject::Account(2), &twice, &alice(), &request).await,
        Err(SharingError::Duplicate)
    ));
    let unknown = [(photos, Some(Permission::Read)), (ShareId::new(99), Some(Permission::Read))];
    assert!(matches!(
        s.sharing.set_subject_access(Subject::Account(2), &unknown, &alice(), &request).await,
        Err(SharingError::ShareNotFound)
    ));
    assert_eq!(s.host.calls().len(), calls);
}

#[tokio::test]
async fn personal_folders_are_given_one_by_one_and_only_their_owner_reaches_them() {
    let s = setup();
    let request = RequestContext::default();
    let bob = Actor { account_id: 2, name: "bob".into() };
    let off = s.sharing.set_account_home(1, true, None, &alice(), &request).await;
    assert!(matches!(off, Err(SharingError::Refused(_))), "personal folders are off: {off:?}");

    let settings = s.sharing.enable_homes("tank", &alice(), &request).await.unwrap();
    assert_eq!(settings.location.unwrap().volume, 1);
    let quota = Some(5 << 30);
    s.sharing.set_account_home(1, true, quota, &alice(), &request).await.unwrap();

    assert!(
        s.host.calls().ends_with(&[
            "prepare_homes tank 1".to_owned(),
            "apply 0".to_owned(),
            format!("set_home tank 1 alice {quota:?}"),
            "apply 0".to_owned()
        ]),
        "{:?}",
        s.host.calls()
    );
    let config = s.host.configs.lock().unwrap().last().unwrap().clone();
    assert_eq!(config.homes, Some(crate::domain::HomesSpec { volume: 1, users: vec!["alice".into()] }));
    assert_eq!(s.files.home(&alice()).await.unwrap(), Some(1));
    assert_eq!(s.files.home(&bob).await.unwrap(), None);

    s.files.list(&alice(), crate::domain::HOME_SHARE, FilePath::root(), None, &request).await.unwrap();
    assert!(s.file_host.calls().contains(&"list alice home:".to_owned()), "{:?}", s.file_host.calls());
    let strangers = s.files.list(&bob, crate::domain::HOME_SHARE, FilePath::root(), None, &request).await;
    assert!(matches!(strangers, Err(FilesError::ShareNotFound)));
    let snapshot =
        s.files.list(&alice(), crate::domain::HOME_SHARE, FilePath::root(), Some("x"), &request).await;
    assert!(matches!(snapshot, Err(FilesError::Invalid(_))));

    let small = s.sharing.set_account_home(1, true, Some(1024), &alice(), &request).await;
    assert!(matches!(small, Err(SharingError::InvalidSettings(_))));
    s.sharing.set_account_home(1, false, None, &alice(), &request).await.unwrap();
    assert_eq!(s.files.home(&alice()).await.unwrap(), None, "taken away: files stay, nobody reaches them");
    s.sharing.disable_homes(&alice(), &request).await.unwrap();
    assert_eq!(s.host.configs.lock().unwrap().last().unwrap().homes, None);
}

#[tokio::test]
async fn a_denied_account_keeps_out_although_its_group_may_write() {
    let s = setup();
    let mut photos = input("photos", "tank");
    photos.access = vec![rule(Subject::Group(10), Access::ReadWrite), deny(Subject::Account(1))];
    photos.nfs = vec![];

    s.sharing.create(photos, &alice(), &RequestContext::default()).await.unwrap();

    assert!(
        s.host.calls().contains(&"access 1 photos [@family:rw,alice:-] -[]".to_owned()),
        "{:?}",
        s.host.calls()
    );
    let seen = s.sharing.subject_access(Subject::Account(1)).await.unwrap();
    assert_eq!((seen[0].rule, seen[0].effective), (Some(Permission::Deny), None));
}
