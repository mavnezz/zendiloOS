use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_jobs::testing::{FixedClock, MemoryJobs};
use zendiloos_jobs::{Job, JobState, Jobs};
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, PortError, RequestContext};

use super::ports::{Directory, Names};
use super::{
    FilesError, FilesUseCases, NewShareInput, ShareChanges, SharingError, SharingPorts, SharingUseCases,
    Transfer,
};
use crate::domain::{Access, AccessRule, FilePath, Principal, ShareId, Subject};
use crate::testing::{Files, Host, Shares, Volumes};

struct Static;

#[async_trait]
impl Directory for Static {
    async fn names(&self) -> Result<Names, PortError> {
        let mut names = Names::default();
        names.accounts.insert(1, "alice".into());
        names.accounts.insert(2, "bob".into());
        names.groups.insert(10, "family".into());
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

struct Setup {
    sharing: SharingUseCases,
    files: FilesUseCases,
    host: Arc<Host>,
    file_host: Arc<Files>,
    jobs: Jobs,
}

fn setup() -> Setup {
    let host = Arc::new(Host::default());
    let file_host = Arc::new(Files::default());
    let jobs = Jobs::new(Arc::new(MemoryJobs::default()), Arc::new(FixedClock));
    let ports = SharingPorts {
        shares: Arc::new(Shares::default()),
        directory: Arc::new(Static),
        volumes: Arc::new(Volumes),
        host: host.clone(),
        files: file_host.clone(),
        jobs: jobs.clone(),
        audit: Arc::new(NoAudit),
        clock: Arc::new(FixedClock),
    };
    Setup {
        sharing: SharingUseCases::new(ports.clone()),
        files: FilesUseCases::new(ports),
        host,
        file_host,
        jobs,
    }
}

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn rule(subject: Subject, access: Access) -> AccessRule {
    AccessRule { subject, access }
}

fn input(name: &str, pool: &str) -> NewShareInput {
    NewShareInput {
        name: name.into(),
        description: Some(" Holiday photos ".into()),
        pool: pool.into(),
        recycle_bin: true,
        access: vec![rule(Subject::Group(10), Access::ReadWrite), rule(Subject::Account(2), Access::Read)],
        nfs: vec![("192.168.1.0/24".into(), false)],
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
    assert_eq!(s.host.calls(), ["create tank 1 photos", "access 1 photos [@family:rw,bob:r] -[]", "apply 1"]);
    let config = s.host.configs.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        config.smb[0].access,
        [
            (Principal::Group("family".into()), Access::ReadWrite),
            (Principal::User("bob".into()), Access::Read)
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
    assert_eq!(s.host.calls(), ["create tank 1 photos", "destroy tank 1 photos"]);
    assert!(s.sharing.list().await.unwrap().is_empty());
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
        &s.host.calls()[3..],
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
    assert_eq!(&s.host.calls()[3..], ["apply 0", "destroy tank 1 photos"]);
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

    let listing = s.files.list(&bob(), id, file("2024"), &request).await.unwrap();
    assert_eq!(listing.entries.len(), 1);
    assert!(matches!(
        s.files.list(&carol(), id, FilePath::root(), &request).await,
        Err(FilesError::ShareNotFound)
    ));
    assert!(matches!(
        s.files.list(&bob(), ShareId::new(99), FilePath::root(), &request).await,
        Err(FilesError::ShareNotFound)
    ));
    assert!(matches!(
        s.files.open(&bob(), id, file("missing.jpg"), &request).await,
        Err(FilesError::NotFound)
    ));
    assert_eq!(s.file_host.calls(), ["list bob photos:2024", "open bob photos:missing.jpg"], "as the user");
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
