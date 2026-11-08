//! Two-factor sign-in through the real router: setting the authenticator
//! app up, the second step of a sign-in, turning it off and an
//! administrator's reset.

use std::time::Duration;

use axum::http::{Method, StatusCode, header, request};
use serde_json::{Value, json};
use zendiloos_identity::domain::{Role, TotpSecret, time_step};
use zendiloos_identity::testing::FakeCodes;
use zendiloos_kernel::Clock;

use crate::tests::{
    Harness, PASSWORD, Reply, Session, assert_problem, changing, empty, get, json, post, with_cookie,
    with_csrf,
};

/// Alice's authenticator app, as far as the tests need it: its secret.
struct App(TotpSecret);

impl App {
    /// The code it shows now, `steps` time steps on (or back).
    fn code(&self, h: &Harness, steps: i64) -> String {
        let step = time_step(h.identity.clock.now()).checked_add_signed(steps).unwrap();
        FakeCodes::code_for(&self.0, step)
    }
}

fn from(builder: request::Builder, address: &str) -> request::Builder {
    builder.header("x-real-ip", address)
}

async fn login(h: &Harness, name: &str) -> Reply {
    h.send(json(from(post("/api/v1/auth/login"), "192.0.2.1"), json!({ "name": name, "password": PASSWORD })))
        .await
}

async fn second_factor(h: &Harness, token: &Value, code: &str) -> Reply {
    let body = json!({ "token": token, "code": code });
    h.send(json(from(post("/api/v1/auth/login/second-factor"), "192.0.2.1"), body)).await
}

/// The token of Alice's sign-in waiting for its code.
async fn password_step(h: &Harness) -> Value {
    let reply = login(h, "alice").await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{:?}", reply.body);
    reply.body["token"].clone()
}

/// Alice, signed in, with her authenticator app set up through the API; the
/// clock moved on to a step whose code is not taken yet.
async fn alice_with_two_factor(h: &Harness) -> (Session, App, Vec<String>) {
    let alice = h.sign_in("alice", Role::User).await;
    let setup = h.send(empty(with_csrf(post("/api/v1/auth/two-factor/setup"), &alice))).await;
    assert_eq!(setup.status, StatusCode::OK, "{:?}", setup.body);
    let app = App(TotpSecret::from_base32(setup.body["secret"].as_str().unwrap()).unwrap());
    let body = json!({ "code": app.code(h, 0) });
    let confirmed = h.send(json(with_csrf(post("/api/v1/auth/two-factor/confirm"), &alice), body)).await;
    assert_eq!(confirmed.status, StatusCode::OK, "{:?}", confirmed.body);
    let codes = confirmed.body["recovery_codes"].as_array().unwrap();
    let codes = codes.iter().map(|code| code.as_str().unwrap().to_owned()).collect();
    h.identity.clock.advance(Duration::from_secs(30));
    (alice, app, codes)
}

fn id_of(h: &Harness, name: &str) -> i64 {
    h.identity.accounts.all().iter().find(|a| a.name.as_str() == name).unwrap().id.get()
}

#[tokio::test]
async fn an_authenticator_app_is_set_up_with_its_first_code() {
    let h = Harness::new();
    let alice = h.sign_in("alice", Role::User).await;
    let status = h.send(empty(with_cookie(get("/api/v1/auth/two-factor"), &alice))).await;
    assert_eq!(status.body, json!({ "enabled": false, "recovery_codes_left": 0 }));
    let without_csrf = h.send(empty(with_cookie(post("/api/v1/auth/two-factor/setup"), &alice))).await;
    assert_problem(&without_csrf, StatusCode::FORBIDDEN, "csrf_rejected");

    let setup = h.send(empty(with_csrf(post("/api/v1/auth/two-factor/setup"), &alice))).await;

    let secret = setup.body["secret"].as_str().unwrap();
    assert_eq!(secret.len(), 32);
    assert_eq!(
        setup.body["uri"].as_str().unwrap(),
        format!(
            "otpauth://totp/zendiloOS:alice?secret={secret}&issuer=zendiloOS&algorithm=SHA1&digits=6&period=30"
        )
    );
    let app = App(TotpSecret::from_base32(secret).unwrap());
    let confirm = |code: String| {
        json(with_csrf(post("/api/v1/auth/two-factor/confirm"), &alice), json!({ "code": code }))
    };
    let wrong = h.send(confirm("000000".to_owned())).await;
    assert_problem(&wrong, StatusCode::UNPROCESSABLE_ENTITY, "invalid_code");

    let confirmed = h.send(confirm(app.code(&h, 0))).await;

    assert_eq!(confirmed.status, StatusCode::OK, "{:?}", confirmed.body);
    assert_eq!(confirmed.body["recovery_codes"].as_array().unwrap().len(), 10);
    let status = h.send(empty(with_cookie(get("/api/v1/auth/two-factor"), &alice))).await;
    assert_eq!(status.body, json!({ "enabled": true, "recovery_codes_left": 10 }));
    let again = h.send(empty(with_csrf(post("/api/v1/auth/two-factor/setup"), &alice))).await;
    assert_problem(&again, StatusCode::CONFLICT, "two_factor_on");
}

#[tokio::test]
async fn confirming_without_a_setup_is_a_conflict() {
    let h = Harness::new();
    let alice = h.sign_in("alice", Role::User).await;

    let body = json!({ "code": "123456" });
    let reply = h.send(json(with_csrf(post("/api/v1/auth/two-factor/confirm"), &alice), body)).await;

    assert_problem(&reply, StatusCode::CONFLICT, "no_two_factor_setup");
}

#[tokio::test]
async fn with_a_second_factor_the_password_alone_sets_no_cookie() {
    let h = Harness::new();
    let (_, app, recovery) = alice_with_two_factor(&h).await;

    let reply = login(&h, "alice").await;

    assert_eq!(reply.status, StatusCode::ACCEPTED, "{:?}", reply.body);
    assert!(reply.headers.get(header::SET_COOKIE).is_none(), "no session yet");
    assert_eq!(reply.body["token"].as_str().unwrap().len(), "token-1".len());
    let expires_at = h.identity.clock.now().unix_seconds() + 300;
    assert_eq!(reply.body["expires_at"], expires_at);
    let wrong = second_factor(&h, &reply.body["token"], "000000").await;
    assert_problem(&wrong, StatusCode::UNPROCESSABLE_ENTITY, "invalid_code");

    let signed_in = second_factor(&h, &reply.body["token"], &app.code(&h, 0)).await;

    assert_eq!(signed_in.status, StatusCode::OK, "{:?}", signed_in.body);
    assert_eq!(signed_in.body["account"]["name"], "alice");
    let cookie = signed_in.header(header::SET_COOKIE).split(';').next().unwrap().to_owned();
    let session = h.send(empty(get("/api/v1/auth/session").header(header::COOKIE, &cookie))).await;
    assert_eq!(session.status, StatusCode::OK, "the session works");
    let token = password_step(&h).await;
    let by_recovery = second_factor(&h, &token, &recovery[0].to_uppercase()).await;
    assert_eq!(by_recovery.status, StatusCode::OK, "{:?}", by_recovery.body);
    let token = password_step(&h).await;
    let again = second_factor(&h, &token, &recovery[0]).await;
    assert_problem(&again, StatusCode::UNPROCESSABLE_ENTITY, "invalid_code");
}

#[tokio::test]
async fn a_code_is_taken_once() {
    let h = Harness::new();
    let (_, app, _) = alice_with_two_factor(&h).await;
    let code = app.code(&h, 0);
    let first = password_step(&h).await;
    assert_eq!(second_factor(&h, &first, &code).await.status, StatusCode::OK);

    let second = password_step(&h).await;
    let replayed = second_factor(&h, &second, &code).await;

    assert_problem(&replayed, StatusCode::UNPROCESSABLE_ENTITY, "invalid_code");
}

#[tokio::test]
async fn a_sign_in_waits_for_its_code_a_while_from_its_address_only() {
    let h = Harness::new();
    let (_, app, _) = alice_with_two_factor(&h).await;
    let token = password_step(&h).await;

    let body = json!({ "token": token, "code": app.code(&h, 0) });
    let elsewhere = h.send(json(from(post("/api/v1/auth/login/second-factor"), "192.0.2.9"), body)).await;
    assert_problem(&elsewhere, StatusCode::UNAUTHORIZED, "sign_in_expired");
    let foreign = h
        .send(json(
            from(post("/api/v1/auth/login/second-factor"), "192.0.2.1")
                .header("sec-fetch-site", "cross-site"),
            json!({ "token": token, "code": app.code(&h, 0) }),
        ))
        .await;
    assert_problem(&foreign, StatusCode::FORBIDDEN, "csrf_rejected");

    h.identity.clock.advance(Duration::from_secs(5 * 60));

    let expired = second_factor(&h, &token, &app.code(&h, 0)).await;
    assert_problem(&expired, StatusCode::UNAUTHORIZED, "sign_in_expired");
}

#[tokio::test]
async fn three_wrong_codes_end_a_sign_in() {
    let h = Harness::new();
    let (_, app, _) = alice_with_two_factor(&h).await;
    let token = password_step(&h).await;
    for _ in 0..3 {
        let wrong = second_factor(&h, &token, "000000").await;
        assert_problem(&wrong, StatusCode::UNPROCESSABLE_ENTITY, "invalid_code");
    }

    let ended = second_factor(&h, &token, &app.code(&h, 0)).await;

    assert_problem(&ended, StatusCode::UNAUTHORIZED, "sign_in_expired");
}

#[tokio::test]
async fn wrong_codes_are_throttled_like_wrong_passwords() {
    let h = Harness::new();
    let (_, _, _) = alice_with_two_factor(&h).await;
    for _ in 0..5 {
        let token = password_step(&h).await;
        let _ = second_factor(&h, &token, "000000").await;
    }

    let locked = login(&h, "alice").await;

    assert_problem(&locked, StatusCode::TOO_MANY_REQUESTS, "too_many_attempts");
    assert!(locked.header(header::RETRY_AFTER).parse::<u64>().unwrap() > 0);
}

#[tokio::test]
async fn new_recovery_codes_and_turning_it_off() {
    let h = Harness::new();
    let (alice, app, old) = alice_with_two_factor(&h).await;

    let body = json!({ "code": app.code(&h, 0) });
    let renewed = h.send(json(with_csrf(post("/api/v1/auth/two-factor/recovery-codes"), &alice), body)).await;
    assert_eq!(renewed.status, StatusCode::OK, "{:?}", renewed.body);
    let new = renewed.body["recovery_codes"].as_array().unwrap();
    assert_eq!(new.len(), 10);
    assert!(new.iter().all(|code| !old.contains(&code.as_str().unwrap().to_owned())));

    let disable = |password: &str, code: &str| {
        json(
            with_csrf(post("/api/v1/auth/two-factor/disable"), &alice),
            json!({ "password": password, "code": code }),
        )
    };
    let wrong_password = h.send(disable("wrong password", new[0].as_str().unwrap())).await;
    assert_problem(&wrong_password, StatusCode::UNPROCESSABLE_ENTITY, "wrong_password");
    let old_code = h.send(disable(PASSWORD, &old[0])).await;
    assert_problem(&old_code, StatusCode::UNPROCESSABLE_ENTITY, "invalid_code");
    let off = h.send(disable(PASSWORD, new[0].as_str().unwrap())).await;
    assert_eq!(off.status, StatusCode::NO_CONTENT, "{:?}", off.body);

    let status = h.send(empty(with_cookie(get("/api/v1/auth/two-factor"), &alice))).await;
    assert_eq!(status.body["enabled"], false);
    assert_eq!(login(&h, "alice").await.status, StatusCode::OK, "the password alone signs in");
    let again = h.send(disable(PASSWORD, "123456")).await;
    assert_problem(&again, StatusCode::CONFLICT, "two_factor_off");
}

#[tokio::test]
async fn admins_see_who_signs_in_with_a_code_and_reset_it() {
    let h = Harness::new();
    let admin = h.sign_in("admin", Role::Admin).await;
    let (alice, _, _) = alice_with_two_factor(&h).await;
    let bob = h.sign_in("bob", Role::User).await;
    let (alice_id, bob_id) = (id_of(&h, "alice"), id_of(&h, "bob"));

    let accounts = h.send(empty(with_cookie(get("/api/v1/accounts"), &admin))).await;
    let two_factor: Vec<(&str, bool)> = accounts
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["name"].as_str().unwrap(), a["two_factor"].as_bool().unwrap()))
        .collect();
    assert_eq!(two_factor, [("admin", false), ("alice", true), ("bob", false)]);

    let reset = |session: &Session, id: i64| {
        empty(with_csrf(changing(Method::DELETE, &format!("/api/v1/accounts/{id}/two-factor")), session))
    };
    assert_problem(&h.send(reset(&bob, alice_id)).await, StatusCode::FORBIDDEN, "forbidden");
    assert_problem(&h.send(reset(&admin, id_of(&h, "admin"))).await, StatusCode::CONFLICT, "own_account");
    assert_problem(&h.send(reset(&admin, bob_id)).await, StatusCode::CONFLICT, "two_factor_off");
    assert_problem(&h.send(reset(&admin, 999)).await, StatusCode::NOT_FOUND, "not_found");

    let done = h.send(reset(&admin, alice_id)).await;

    assert_eq!(done.status, StatusCode::NO_CONTENT, "{:?}", done.body);
    let gone = h.send(empty(with_cookie(get("/api/v1/auth/session"), &alice))).await;
    assert_problem(&gone, StatusCode::UNAUTHORIZED, "unauthenticated");
    assert_eq!(login(&h, "alice").await.status, StatusCode::OK);
    let entry =
        h.identity.audit.entries().into_iter().find(|e| e.action == "identity.two_factor_reset").unwrap();
    assert_eq!((entry.actor.as_deref(), entry.target.as_deref()), (Some("admin"), Some("alice")));
}
