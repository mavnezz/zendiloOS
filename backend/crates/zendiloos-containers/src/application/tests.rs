use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use zendiloos_jobs::testing::FixedClock;
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, PortError, RequestContext};

use super::ports::ProjectAction;
use super::{ContainersError, NewProject};
use crate::domain::{
    ContainerAction, Origin, ProjectTemplate, Risk, UpdateSchedule, UpdateSettings, UpdateState,
};
use crate::testing::TestContainers;

#[derive(Default)]
struct Audit(Mutex<Vec<AuditEntry>>);

#[async_trait]
impl AuditLog for Audit {
    async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
        self.0.lock().unwrap().push(entry);
        Ok(())
    }
}

impl Audit {
    fn actions(&self) -> Vec<(&'static str, Option<String>, Option<&'static str>)> {
        self.0.lock().unwrap().iter().map(|e| (e.action, e.target.clone(), e.reason)).collect()
    }
}

fn setup() -> (TestContainers, Arc<Audit>) {
    let audit = Arc::new(Audit::default());
    (TestContainers::new(audit.clone(), Arc::new(FixedClock)), audit)
}

fn alice() -> Actor {
    Actor { account_id: 1, name: "alice".into() }
}

fn context() -> RequestContext {
    RequestContext::default()
}

/// Waits until the engine was asked for `call` (or `call` followed by
/// more, such as a deploy's project); jobs run in the background. Says
/// what was asked.
async fn asked(t: &TestContainers, call: &str) -> String {
    for _ in 0..200 {
        if let Some(found) =
            t.engine.calls().into_iter().find(|c| c == call || c.starts_with(&format!("{call} ")))
        {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("never asked for {call:?}: {:?}", t.engine.calls());
}

fn new_project(name: &str, compose: &str, accept_risks: bool) -> NewProject {
    NewProject {
        name: name.into(),
        share_id: 1,
        folder: " docker/tesla/ ".into(),
        compose: compose.into(),
        start: true,
        accept_risks,
        template: None,
    }
}

#[tokio::test]
async fn the_folders_a_project_mounts_are_made_by_whoever_deploys_it() {
    let (t, _) = setup();
    let compose = "services:\n  app:\n    image: nginx\n    volumes:\n      - ./config:/config\n      - ./data/db:/db\n";
    let template = ProjectTemplate { source: 1, app: "jellyfin".into() };
    let new = NewProject { template: Some(template.clone()), ..new_project("tesla", compose, false) };

    t.containers.create_project(new, &alice(), &context()).await.unwrap();

    let made = t.storage.folders.lock().unwrap().clone();
    let by_alice = |path: &str| (1, path.to_owned(), "alice".to_owned());
    assert_eq!(made, [by_alice("docker/tesla/config"), by_alice("docker/tesla/data/db")]);
    let project = t.containers.project("tesla", &context()).await.unwrap();
    let Origin::Own(own) = project.origin else { panic!("{:?}", project.origin) };
    assert_eq!(own.template, Some(template), "the App Center finds it again");

    let changed = format!("{compose}      - ./cache:/cache\n");
    t.containers.change_compose("tesla", &changed, false, &alice(), &context()).await.unwrap();
    assert!(t.storage.folders.lock().unwrap()[2..].contains(&by_alice("docker/tesla/cache")));
}

#[tokio::test]
async fn the_overview_counts_what_runs() {
    let (t, _) = setup();
    t.containers.sample().await.unwrap();

    let overview = t.containers.overview(&context()).await.unwrap();

    assert_eq!((overview.projects_running, overview.projects), (2, 2));
    assert_eq!((overview.containers_running, overview.containers), (2, 4));
    assert_eq!((overview.images, overview.data_bytes), (5, 5_001_000));
    assert_eq!(overview.cpu_share, Some(5.0), "two containers at 10 % of one of four processors");
}

#[tokio::test]
async fn containers_carry_their_use_while_they_run() {
    let (t, audit) = setup();
    t.containers.sample().await.unwrap();

    let containers = t.containers.containers(&context()).await.unwrap();

    let names: Vec<_> = containers.iter().map(|c| (c.container.name.as_str(), c.usage.is_some())).collect();
    assert_eq!(names, [("kuma-1", true), ("lonely", false), ("stack-db", false), ("stack-web", true)]);
    t.containers.control_container("lonely-id", ContainerAction::Start, &alice(), &context()).await.unwrap();
    assert_eq!(audit.actions().last().unwrap().0, "containers.container_start");
    let unknown = t.containers.control_container("nope", ContainerAction::Stop, &alice(), &context()).await;
    assert!(matches!(unknown, Err(ContainersError::NotFound)), "{unknown:?}");
}

#[tokio::test]
async fn images_name_who_uses_them_and_only_unused_ones_go() {
    let (t, audit) = setup();

    let images = t.containers.images(&context()).await.unwrap();

    let nginx = images.iter().find(|i| i.image.id == "sha256:nginx").unwrap();
    assert_eq!(nginx.used_by, ["stack-web"]);
    assert_eq!(images[0].image.id, "sha256:unused", "newest first");
    let used = t.containers.remove_image("sha256:nginx", &alice(), &context()).await;
    assert!(matches!(used, Err(ContainersError::Failed(_))), "{used:?}");
    t.containers.remove_image("sha256:unused", &alice(), &context()).await.unwrap();
    let actions: Vec<_> = audit.actions().iter().map(|a| a.0).collect();
    assert_eq!(actions, ["containers.image_remove", "containers.image_remove"]);
    let job = t.containers.pull_image(" redis:7 ", &alice(), &context()).await.unwrap();
    assert_eq!(job.kind, "containers.image_pull");
    asked(&t, "pull redis:7").await;
    assert!(matches!(
        t.containers.pull_image("redis 7", &alice(), &context()).await,
        Err(ContainersError::Invalid(_))
    ));
}

#[tokio::test]
async fn dockers_own_networks_come_first_and_stay() {
    let (t, _) = setup();
    t.containers
        .create_network("proxy_net", Some("172.30.0.0/24"), None, &alice(), &context())
        .await
        .unwrap();

    let networks = t.containers.networks(&context()).await.unwrap();

    let names: Vec<_> = networks.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["bridge", "proxy_net", "stack_default"]);
    let own = t.containers.remove_network("bridge", &alice(), &context()).await;
    assert!(matches!(own, Err(ContainersError::Refused(_))), "{own:?}");
    let gateway = t.containers.create_network("x", None, Some("10.0.0.1"), &alice(), &context()).await;
    assert!(matches!(gateway, Err(ContainersError::Invalid(_))), "{gateway:?}");
    t.containers.remove_network("proxy_net", &alice(), &context()).await.unwrap();
}

#[tokio::test]
async fn a_project_that_reaches_beyond_its_containers_needs_consent() {
    let (t, audit) = setup();
    let compose = "services:\n  app:\n    image: nginx\n    privileged: true\n";

    let refused =
        t.containers.create_project(new_project("tesla", compose, false), &alice(), &context()).await;

    let Err(ContainersError::Risky(risks)) = refused else { panic!("{refused:?}") };
    assert_eq!(risks, [Risk::Privileged { service: "app".into() }]);
    assert!(t.storage.files.lock().unwrap().is_empty(), "nothing written");

    let job =
        t.containers.create_project(new_project("tesla", compose, true), &alice(), &context()).await.unwrap();

    assert_eq!(job.unwrap().kind, "containers.project_deploy");
    assert_eq!(
        t.storage.files.lock().unwrap().get(&(1, "docker/tesla/compose.yaml".to_owned())).map(String::as_str),
        Some(compose)
    );
    let deployed = asked(&t, "deploy tesla pull=false").await;
    assert!(deployed.contains(r#""privileged":true"#), "what was confirmed runs: {deployed}");
    assert!(t.engine.calls().contains(&"check tesla /volume1/docker/docker/tesla".to_owned()));
    let created = audit.actions().into_iter().find(|a| a.0 == "containers.project_create").unwrap();
    assert_eq!(created.2, Some("risks_accepted"));
    let project = t.containers.project("tesla", &context()).await.unwrap();
    assert!(matches!(project.origin, Origin::Own(_)));
}

#[tokio::test]
async fn projects_need_a_free_name_a_readable_file_and_a_writable_share() {
    let (t, _) = setup();
    let plain = "services:\n  app:\n    image: nginx\n";

    let taken = t.containers.create_project(new_project("stack", plain, false), &alice(), &context()).await;
    assert!(matches!(taken, Err(ContainersError::NameTaken)), "{taken:?}");
    let broken =
        t.containers.create_project(new_project("tesla", "broken: [", false), &alice(), &context()).await;
    assert!(matches!(broken, Err(ContainersError::Refused(message)) if message.contains("mapping values")));
    let bob = Actor { account_id: 2, name: "bob".into() };
    let denied = t.containers.create_project(new_project("tesla", plain, false), &bob, &context()).await;
    assert!(matches!(denied, Err(ContainersError::Denied)), "{denied:?}");
    let badly = t.containers.create_project(new_project("Tesla", plain, false), &alice(), &context()).await;
    assert!(matches!(badly, Err(ContainersError::InvalidName(_))), "{badly:?}");
    let resting = NewProject { start: false, ..new_project("tesla", plain, false) };
    assert!(t.containers.create_project(resting, &alice(), &context()).await.unwrap().is_none());
    assert!(!t.engine.calls().iter().any(|c| c.starts_with("deploy")), "not started");
}

#[tokio::test]
async fn an_own_projects_file_is_changed_and_deployed_again() {
    let (t, _) = setup();
    let plain = "services:\n  app:\n    image: nginx\n";
    let resting = NewProject { start: false, ..new_project("tesla", plain, false) };
    t.containers.create_project(resting, &alice(), &context()).await.unwrap();

    let source = t.containers.project_compose("tesla", &alice(), &context()).await.unwrap();
    assert!(source.editable);
    assert_eq!(source.files, [("/volume1/docker/docker/tesla/compose.yaml".to_owned(), plain.to_owned())]);

    let changed = "services:\n  app:\n    image: nginx:1.27\n";
    t.containers.change_compose("tesla", changed, false, &alice(), &context()).await.unwrap();
    let deployed = asked(&t, "deploy tesla pull=false").await;
    assert!(deployed.contains("nginx:1.27"), "{deployed}");
    let source = t.containers.project_compose("tesla", &alice(), &context()).await.unwrap();
    assert_eq!(source.files[0].1, changed);
    let foreign = t.containers.change_compose("stack", changed, false, &alice(), &context()).await;
    assert!(matches!(foreign, Err(ContainersError::NotOwn)), "{foreign:?}");
}

#[tokio::test]
async fn an_own_project_without_containers_still_goes_down_when_deleted() {
    let (t, _) = setup();
    let plain = "services:\n  app:\n    image: nginx\n";
    let resting = NewProject { start: false, ..new_project("tesla", plain, false) };
    t.containers.create_project(resting, &alice(), &context()).await.unwrap();

    t.containers.delete_project("tesla", &alice(), &context()).await.unwrap();
    asked(&t, "project down tesla").await;
}

#[tokio::test]
async fn a_file_changed_in_the_share_does_not_run() {
    let (t, _) = setup();
    let plain = "services:\n  app:\n    image: nginx\n";
    t.containers.create_project(new_project("tesla", plain, false), &alice(), &context()).await.unwrap();
    asked(&t, "deploy tesla pull=false").await;
    // Someone who may write the share changes the file there.
    let changed = "services:\n  app:\n    image: nginx\n    privileged: true\n";
    t.storage.files.lock().unwrap().insert((1, "docker/tesla/compose.yaml".to_owned()), changed.to_owned());

    t.containers.redeploy("tesla", true, &alice(), &context()).await.unwrap();

    let deployed = asked(&t, "deploy tesla pull=true").await;
    assert!(deployed.contains(r#""privileged":false"#), "what was confirmed runs: {deployed}");
    let source = t.containers.project_compose("tesla", &alice(), &context()).await.unwrap();
    assert_eq!(source.files[0].1, plain, "the editor shows what runs");
}

#[tokio::test]
async fn a_project_made_before_runs_once_its_file_is_saved() {
    let (t, _) = setup();
    let plain = "services:\n  app:\n    image: nginx\n";
    let resting = NewProject { start: false, ..new_project("tesla", plain, false) };
    t.containers.create_project(resting, &alice(), &context()).await.unwrap();
    // As a project made before the confirmed file was kept.
    for project in t.projects.0.lock().unwrap().iter_mut() {
        (project.compose, project.config) = (None, None);
    }
    t.storage.files.lock().unwrap().insert((1, "docker/tesla/compose.yaml".to_owned()), plain.to_owned());

    let refused = t.containers.redeploy("tesla", true, &alice(), &context()).await;
    assert!(matches!(refused, Err(ContainersError::Unconfirmed)), "{refused:?}");
    let source = t.containers.project_compose("tesla", &alice(), &context()).await.unwrap();
    assert_eq!(source.files[0].1, plain, "read from the share to be saved");

    t.containers.change_compose("tesla", plain, false, &alice(), &context()).await.unwrap();
    asked(&t, "deploy tesla pull=false").await;
    t.containers.redeploy("tesla", true, &alice(), &context()).await.unwrap();
    asked(&t, "deploy tesla pull=true").await;
}

#[tokio::test]
async fn a_redeploy_pulls_only_when_asked_and_its_job_tells_what_compose_wrote() {
    let (t, _) = setup();
    let plain = "services:\n  app:\n    image: nginx\n";
    let resting = NewProject { start: false, ..new_project("tesla", plain, false) };
    t.containers.create_project(resting, &alice(), &context()).await.unwrap();

    let job = t.containers.redeploy("tesla", false, &alice(), &context()).await.unwrap();
    asked(&t, "deploy tesla pull=false").await;
    let ended = finished(&t, job.id).await;
    assert_eq!(ended.output, [" Container tesla-app  Started"]);

    let job = t.containers.redeploy("tesla", true, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&t, job.id).await.output, [" app Pulled", " Container tesla-app  Started"]);

    // Made elsewhere: what Docker writes as it pulls and brings it up.
    let job = t.containers.redeploy("stack", false, &alice(), &context()).await.unwrap();
    assert_eq!(finished(&t, job.id).await.output, ["up stack"]);
}

/// The job `id` once it ended, as stored.
async fn finished(t: &TestContainers, id: zendiloos_jobs::JobId) -> zendiloos_jobs::Job {
    for _ in 0..200 {
        if let Some(job) = t.jobs.all().into_iter().find(|job| job.id == id && job.finished_at.is_some()) {
            // Output published after the end would come within a quarter second.
            tokio::time::sleep(Duration::from_millis(300)).await;
            return t.jobs.all().into_iter().find(|j| j.id == id).unwrap_or(job);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("job {id:?} did not end");
}

#[tokio::test]
async fn apps_of_catalogs_are_left_to_the_app_center() {
    let (t, _) = setup();

    for refused in [
        t.containers.redeploy("kuma", false, &alice(), &context()).await.map(|_| ()),
        t.containers.delete_project("kuma", &alice(), &context()).await.map(|_| ()),
        t.containers.project_compose("kuma", &alice(), &context()).await.map(|_| ()),
        t.containers.change_compose("kuma", "x", true, &alice(), &context()).await.map(|_| ()),
    ] {
        assert!(matches!(refused, Err(ContainersError::AppProject)), "{refused:?}");
    }
    t.containers.control_project("kuma", ProjectAction::Restart, &alice(), &context()).await.unwrap();
    assert_eq!(t.engine.calls(), ["project restart kuma"]);
}

#[tokio::test]
async fn projects_made_elsewhere_are_shown_updated_and_removed() {
    let (t, audit) = setup();

    let source = t.containers.project_compose("stack", &alice(), &context()).await.unwrap();
    assert!(!source.editable);
    assert_eq!(source.files[0].0, "/opt/stack/compose.yml");

    t.containers.redeploy("stack", true, &alice(), &context()).await.unwrap();
    asked(&t, "project up stack").await;
    assert!(t.engine.calls().contains(&"project pull stack".to_owned()), "images fetched first");

    t.containers.delete_project("stack", &alice(), &context()).await.unwrap();
    asked(&t, "project down stack").await;
    let logs = t.containers.project_logs("stack", 100, &context()).await.unwrap();
    assert!(logs.contains("ready"));
    for _ in 0..200 {
        if audit.actions().iter().any(|a| a.0 == "containers.project_delete") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("not audited: {:?}", audit.actions());
}

#[tokio::test]
async fn the_history_keeps_every_sample() {
    let (t, _) = setup();
    t.containers.sample().await.unwrap();
    t.containers.sample().await.unwrap();

    let moments = t.containers.usage_history(3600, &["kuma-1".to_owned()]);

    assert!(!moments.is_empty());
    assert!(moments.iter().all(|m| m.points.iter().all(|p| p.container == "kuma-1")));
}

#[tokio::test]
async fn newer_images_are_found_and_their_projects_updated() {
    let (t, audit) = setup();

    let report = t.containers.check_updates(&context()).await.unwrap();

    let states: Vec<_> = report.images.iter().map(|i| (i.reference.as_str(), i.state.clone())).collect();
    assert_eq!(states[0], ("alpine", UpdateState::Unknown("the registry does not answer".into())));
    assert_eq!(states[2], ("nginx", UpdateState::Available));
    assert_eq!(states[3], ("postgres", UpdateState::Current));
    assert_eq!(t.containers.update_report(), Some(report));

    let job = t.containers.apply_updates(Some(&alice()), &context()).await.unwrap().expect("a job");
    assert_eq!(job.kind, "containers.update");
    asked(&t, "project up stack").await;
    assert!(t.engine.calls().contains(&"project pull stack".to_owned()));
    assert!(!t.engine.calls().iter().any(|c| c.contains(" kuma")), "the app is the App Center's");
    for _ in 0..200 {
        if audit.actions().iter().any(|a| a.0 == "containers.update") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("not audited");
}

#[tokio::test]
async fn updates_come_on_schedule_only_when_due() {
    let (t, _) = setup();
    let schedule = UpdateSchedule { days: [false, false, true, false, false, false, false], minute: 340 };
    let settings = UpdateSettings { check: true, schedule: Some(schedule) };
    t.containers.set_update_settings(settings, &alice(), &context()).await.unwrap();

    assert!(t.containers.scheduled(2, 339).await.unwrap().is_none());
    assert!(t.engine.calls().is_empty(), "not even checked");
    assert!(t.containers.scheduled(2, 340).await.unwrap().is_some());
    asked(&t, "project up stack").await;

    let unchecked = UpdateSettings { check: false, schedule: Some(schedule) };
    let refused = t.containers.set_update_settings(unchecked, &alice(), &context()).await;
    assert!(matches!(refused, Err(ContainersError::Invalid(_))), "{refused:?}");
}

#[tokio::test]
async fn dockers_data_moves_to_a_pool_with_room() {
    let (t, audit) = setup();

    let location = t.containers.data_location(&context()).await.unwrap();
    assert_eq!(location.current.as_ref().map(|c| c.pool.as_str()), Some("tank"));
    let targets: Vec<_> = location.targets.iter().map(|p| p.pool.as_str()).collect();
    assert_eq!(targets, ["backup", "tiny"]);

    let small = t.containers.move_data("tiny", &alice(), &context()).await;
    assert!(matches!(small, Err(ContainersError::Refused(_))), "{small:?}");
    let job = t.containers.move_data("backup", &alice(), &context()).await.unwrap();
    assert_eq!(job.kind, "containers.data_move");
    for _ in 0..200 {
        if t.data.current.lock().unwrap().as_ref().map(|c| c.pool.clone()) == Some("backup".into()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(t.data.current.lock().unwrap().as_ref().map(|c| c.pool.as_str()), Some("backup"));
    let same = t.containers.move_data("backup", &alice(), &context()).await;
    assert!(matches!(same, Err(ContainersError::NotFound)), "{same:?}");
    let _ = audit;
}
