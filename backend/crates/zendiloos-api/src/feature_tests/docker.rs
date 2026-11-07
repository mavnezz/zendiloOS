//! Docker through the API: containers, images, networks and projects as the
//! web interface shows and changes them, refusals and the engine's errors.

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use zendiloos_identity::domain::Role;

use crate::tests::{
    Harness, Session, assert_problem, changing, empty, get, json, post, with_cookie, with_csrf,
};

async fn admin(h: &Harness) -> Session {
    h.sign_in("alice", Role::Admin).await
}

#[tokio::test]
async fn containers_are_listed_with_their_use_and_logs() {
    let h = Harness::new();
    let admin = admin(&h).await;
    h.containers.sample().await.unwrap();

    let containers = h.send(empty(with_cookie(get("/api/v1/docker/containers"), &admin))).await;
    assert_eq!(containers.status, StatusCode::OK, "{:?}", containers.body);
    let kuma = containers.body.as_array().unwrap().iter().find(|c| c["name"] == "kuma-1").cloned().unwrap();
    assert_eq!(kuma["usage"]["cpu_percent"], json!(10.0));
    assert_eq!(kuma["restart_policy"], "unless-stopped");

    let logs =
        h.send(empty(with_cookie(get("/api/v1/docker/containers/kuma-1-id/logs?lines=20"), &admin))).await;
    assert!(logs.body["text"].as_str().unwrap().contains("said 20 lines"), "{:?}", logs.body);

    let usage = h
        .send(empty(with_cookie(
            get("/api/v1/docker/usage?seconds=10&containers=kuma-1,%20,stack-web"),
            &admin,
        )))
        .await;
    assert_eq!(usage.status, StatusCode::OK, "{:?}", usage.body);
    let points: Vec<&Value> =
        usage.body.as_array().unwrap().iter().flat_map(|m| m["points"].as_array().unwrap()).collect();
    assert!(points.iter().all(|p| p["container"] == "kuma-1" || p["container"] == "stack-web"), "{points:?}");
    assert!(!points.is_empty());
}

#[tokio::test]
async fn containers_are_started_stopped_and_removed() {
    let h = Harness::new();
    let admin = admin(&h).await;
    for action in ["start", "restart", "stop"] {
        let done = h
            .send(empty(with_csrf(post(&format!("/api/v1/docker/containers/lonely-id/{action}")), &admin)))
            .await;
        assert_eq!(done.status, StatusCode::NO_CONTENT, "{action}: {:?}", done.body);
    }
    let removed = h
        .send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/containers/lonely-id"), &admin)))
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);
    let gone = h.send(empty(with_csrf(post("/api/v1/docker/containers/lonely-id/start"), &admin))).await;
    assert_problem(&gone, StatusCode::NOT_FOUND, "not_found");
    let running = h
        .send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/containers/kuma-1-id"), &admin)))
        .await;
    assert_problem(&running, StatusCode::BAD_GATEWAY, "host_failed");
    let odd = h.send(empty(with_csrf(post("/api/v1/docker/containers/lonely-id/explode"), &admin))).await;
    assert_eq!(odd.status, StatusCode::BAD_REQUEST, "{:?}", odd.body);
}

#[tokio::test]
async fn images_are_pulled_removed_and_pruned() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let pulled = h
        .send(json(with_csrf(post("/api/v1/docker/images/pull"), &admin), json!({ "reference": "redis:7" })))
        .await;
    assert_eq!(pulled.status, StatusCode::ACCEPTED, "{:?}", pulled.body);
    assert_eq!(pulled.body["kind"], "containers.image_pull");
    let invalid = h
        .send(json(with_csrf(post("/api/v1/docker/images/pull"), &admin), json!({ "reference": "redis 7" })))
        .await;
    assert_problem(&invalid, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");

    let removed = h
        .send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/images/sha256:unused"), &admin)))
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);
    let used = h
        .send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/images/sha256:nginx"), &admin)))
        .await;
    assert_problem(&used, StatusCode::BAD_GATEWAY, "host_failed");
    let pruned = h.send(empty(with_csrf(post("/api/v1/docker/images/prune"), &admin))).await;
    assert_eq!(pruned.body["text"], "Total reclaimed space: 1MB");
}

#[tokio::test]
async fn networks_are_listed_made_and_removed() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let networks = h.send(empty(with_cookie(get("/api/v1/docker/networks"), &admin))).await;
    let names: Vec<(Value, Value)> = networks
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n["name"].clone(), n["built_in"].clone()))
        .collect();
    assert_eq!(names, [(json!("bridge"), json!(true)), (json!("stack_default"), json!(false))]);
    let made = h
        .send(json(
            with_csrf(post("/api/v1/docker/networks"), &admin),
            json!({ "name": "proxy", "gateway": "10.1.0.1", "subnet": "10.1.0.0/24" }),
        ))
        .await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let removed =
        h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/networks/proxy"), &admin))).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);
    assert!(h.engine.calls().contains(&"network rm proxy".to_owned()));
}

#[tokio::test]
async fn projects_are_shown_controlled_redeployed_and_deleted() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let stack = h.send(empty(with_cookie(get("/api/v1/docker/projects/stack"), &admin))).await;
    assert_eq!(stack.status, StatusCode::OK, "{:?}", stack.body);
    assert_eq!(stack.body["kind"], "foreign");
    let missing = h.send(empty(with_cookie(get("/api/v1/docker/projects/nothing"), &admin))).await;
    assert_problem(&missing, StatusCode::NOT_FOUND, "not_found");

    for action in ["stop", "start", "restart"] {
        let done =
            h.send(empty(with_csrf(post(&format!("/api/v1/docker/projects/stack/{action}")), &admin))).await;
        assert_eq!(done.status, StatusCode::NO_CONTENT, "{action}: {:?}", done.body);
    }
    let logs = h.send(empty(with_cookie(get("/api/v1/docker/projects/stack/logs?lines=5"), &admin))).await;
    assert_eq!(logs.status, StatusCode::OK, "{:?}", logs.body);

    let redeployed = h.send(empty(with_csrf(post("/api/v1/docker/projects/stack/redeploy"), &admin))).await;
    assert_eq!(redeployed.status, StatusCode::ACCEPTED, "{:?}", redeployed.body);
    let app = h.send(empty(with_csrf(post("/api/v1/docker/projects/kuma/redeploy"), &admin))).await;
    assert_problem(&app, StatusCode::CONFLICT, "app_project");
    let deleted =
        h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/projects/stack"), &admin))).await;
    assert_eq!(deleted.status, StatusCode::ACCEPTED, "{:?}", deleted.body);
    let app =
        h.send(empty(with_csrf(changing(Method::DELETE, "/api/v1/docker/projects/kuma"), &admin))).await;
    assert_problem(&app, StatusCode::CONFLICT, "app_project");
}

#[tokio::test]
async fn a_redeploy_pulls_when_asked_and_its_job_follows_what_compose_writes() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let redeploy = |body: Option<Value>| {
        let request = with_csrf(post("/api/v1/docker/projects/stack/redeploy"), &admin);
        match body {
            Some(body) => h.send(json(request, body)),
            None => h.send(empty(request)),
        }
    };

    let pulled = redeploy(Some(json!({ "pull": true }))).await;
    assert_eq!(pulled.status, StatusCode::ACCEPTED, "{:?}", pulled.body);
    assert_eq!(pulled.body["output"], json!([]), "nothing written yet");
    assert_eq!(output(&h, pulled.body["id"].as_i64().unwrap()).await, ["pull stack", "up stack"]);

    let plain = redeploy(None).await;
    assert_eq!(output(&h, plain.body["id"].as_i64().unwrap()).await, ["up stack"], "no body: no pull");

    let wrong = redeploy(Some(json!({ "pull": "yes" }))).await;
    assert!(wrong.status.is_client_error(), "{:?}", wrong.status);
}

/// The output of the Docker app's job `id`, once it ended and what came
/// last was published, as the API shows it.
async fn output(h: &Harness, id: i64) -> Vec<String> {
    let find = || h.docker_jobs.all().into_iter().find(|job| job.id.get() == id).unwrap();
    for _ in 0..200 {
        if find().finished_at.is_some() {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            return crate::routes::jobs::Job::from(find()).output;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("job {id} did not end");
}

#[tokio::test]
async fn an_own_projects_file_is_changed_with_consent_and_a_foreign_one_not() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let plain = "services:\n  app:\n    image: nginx\n";
    let made = h
        .send(json(
            with_csrf(post("/api/v1/docker/projects"), &admin),
            json!({ "name": "tesla", "share_id": 1, "folder": "docker/tesla", "compose": plain, "start": false, "accept_risks": false }),
        ))
        .await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);

    let change = |compose: &str, accept: bool| {
        json(
            with_csrf(changing(Method::PUT, "/api/v1/docker/projects/tesla/compose"), &admin),
            json!({ "compose": compose, "accept_risks": accept }),
        )
    };
    let risky = h.send(change("services:\n  app:\n    privileged: true\n", false)).await;
    assert_problem(&risky, StatusCode::CONFLICT, "compose_risks");
    assert_eq!(risky.body["detail"], "app: privileged");
    let broken = h.send(change("broken: [", true)).await;
    assert_problem(&broken, StatusCode::CONFLICT, "refused");
    let changed = h.send(change("services:\n  app:\n    image: nginx:1.27\n", false)).await;
    assert_eq!(changed.status, StatusCode::ACCEPTED, "{:?}", changed.body);
    let tesla = h.send(empty(with_cookie(get("/api/v1/docker/projects/tesla"), &admin))).await;
    let file = tesla.body["compose_file"].as_str().unwrap();
    assert!(file.ends_with("docker/tesla/compose.yaml"), "where it is in the shares: {file}");
    let foreign = h
        .send(json(
            with_csrf(changing(Method::PUT, "/api/v1/docker/projects/stack/compose"), &admin),
            json!({ "compose": plain }),
        ))
        .await;
    assert_problem(&foreign, StatusCode::CONFLICT, "not_own_project");

    let odd_name = h
        .send(json(
            with_csrf(post("/api/v1/docker/projects"), &admin),
            json!({ "name": "Not A Name", "share_id": 1, "folder": "docker/x", "compose": plain, "start": false, "accept_risks": false }),
        ))
        .await;
    assert_problem(&odd_name, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input");
    let elsewhere = h
        .send(json(
            with_csrf(post("/api/v1/docker/projects"), &admin),
            json!({ "name": "other", "share_id": 99, "folder": "docker/x", "compose": plain, "start": false, "accept_risks": false }),
        ))
        .await;
    assert_problem(&elsewhere, StatusCode::FORBIDDEN, "share_denied");
}

#[tokio::test]
async fn image_update_settings_are_read_back() {
    let h = Harness::new();
    let admin = admin(&h).await;
    let settings = h.send(empty(with_cookie(get("/api/v1/docker/updates/settings"), &admin))).await;
    assert_eq!(settings.status, StatusCode::OK, "{:?}", settings.body);
    assert!(settings.body.get("check").is_some(), "{:?}", settings.body);
}

#[tokio::test]
async fn everything_docker_is_refused_to_those_who_may_not_manage_containers() {
    let h = Harness::new();
    let viewer = h.sign_in("bob", Role::ReadOnly).await;
    for path in [
        "/api/v1/docker/usage",
        "/api/v1/docker/containers",
        "/api/v1/docker/containers/lonely-id/logs",
        "/api/v1/docker/images",
        "/api/v1/docker/networks",
        "/api/v1/docker/places",
        "/api/v1/docker/projects",
        "/api/v1/docker/projects/stack",
        "/api/v1/docker/projects/stack/compose",
        "/api/v1/docker/projects/stack/logs",
        "/api/v1/docker/updates",
        "/api/v1/docker/updates/settings",
        "/api/v1/docker/data",
    ] {
        let reply = h.send(empty(with_cookie(get(path), &viewer))).await;
        assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    }
    for (method, path) in [
        (Method::POST, "/api/v1/docker/containers/lonely-id/start"),
        (Method::DELETE, "/api/v1/docker/containers/lonely-id"),
        (Method::DELETE, "/api/v1/docker/images/sha256:unused"),
        (Method::POST, "/api/v1/docker/images/prune"),
        (Method::DELETE, "/api/v1/docker/networks/stack_default"),
        (Method::DELETE, "/api/v1/docker/projects/stack"),
        (Method::POST, "/api/v1/docker/projects/stack/start"),
        (Method::POST, "/api/v1/docker/projects/stack/redeploy"),
        (Method::POST, "/api/v1/docker/updates/check"),
        (Method::POST, "/api/v1/docker/updates/apply"),
    ] {
        let reply = h.send(empty(with_csrf(changing(method, path), &viewer))).await;
        assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    }
    for (method, path, body) in [
        (Method::POST, "/api/v1/docker/images/pull", json!({ "reference": "redis:7" })),
        (Method::POST, "/api/v1/docker/networks", json!({ "name": "x" })),
        (Method::POST, "/api/v1/docker/compose/check", json!({ "name": "x", "compose": "services: {}" })),
        (
            Method::POST,
            "/api/v1/docker/projects",
            json!({ "name": "x", "share_id": 1, "folder": "x", "compose": "", "start": false, "accept_risks": false }),
        ),
        (Method::PUT, "/api/v1/docker/projects/stack/compose", json!({ "compose": "" })),
        (Method::PUT, "/api/v1/docker/updates/settings", json!({ "check": false, "schedule": null })),
        (Method::POST, "/api/v1/docker/data/move", json!({ "pool": "backup" })),
    ] {
        let reply = h.send(json(with_csrf(changing(method, path), &viewer), body)).await;
        assert_problem(&reply, StatusCode::FORBIDDEN, "forbidden");
    }
}
