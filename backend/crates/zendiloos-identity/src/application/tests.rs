use std::net::IpAddr;
use std::time::Duration;

use zendiloos_kernel::{RequestContext, Secret};

use super::IdentityError;
use super::commands::{ChangePasswordInput, LoginInput, SetupInput};
use zendiloos_kernel::Outcome;

use crate::domain::{AuditAction, Role};
use crate::testing::{FakeHasher, SETUP_CODE, TestIdentity};

const PASSWORD: &str = "correct horse battery";

fn from(last_octet: u8) -> RequestContext {
    RequestContext {
        client_ip: Some(IpAddr::from([192, 0, 2, last_octet])),
        user_agent: Some("test".to_owned()),
        request_id: Some("request-1".to_owned()),
    }
}

fn setup_input(code: &str, name: &str, password: &str) -> SetupInput {
    SetupInput { setup_code: Secret::new(code), name: name.to_owned(), password: Secret::new(password) }
}

fn login_input(name: &str, password: &str) -> LoginInput {
    LoginInput { name: name.to_owned(), password: Secret::new(password) }
}

// Initial setup

#[tokio::test]
async fn prepare_setup_issues_a_code_only_while_no_admin_exists() {
    let t = TestIdentity::new();
    let code = t.use_cases.prepare_setup.execute().await.unwrap().expect("a code");
    assert_eq!(code.reveal(), SETUP_CODE);
    assert!(t.use_cases.setup_status.execute().await.unwrap().required);

    t.add_account("alice", PASSWORD, Role::Admin);

    assert!(t.use_cases.prepare_setup.execute().await.unwrap().is_none());
    assert!(!t.setup_codes.is_present(), "a leftover code is removed");
    assert!(!t.use_cases.setup_status.execute().await.unwrap().required);
}

#[tokio::test]
async fn setup_creates_an_admin_and_signs_them_in() {
    let t = TestIdentity::new();
    t.use_cases.prepare_setup.execute().await.unwrap();

    let signed_in = t
        .use_cases
        .complete_setup
        .execute(setup_input("abcd efgh jklm", "alice", PASSWORD), &from(1))
        .await
        .unwrap();

    let accounts = t.accounts.all();
    assert_eq!(accounts.len(), 1);
    assert_eq!((accounts[0].name.as_str(), accounts[0].role), ("alice", Role::Admin));
    assert_eq!(accounts[0].password_hash, FakeHasher::hash_of(PASSWORD));
    assert_eq!(signed_in.session.account.name.as_str(), "alice");
    assert_eq!(t.sessions.all().len(), 1);
    assert!(!t.setup_codes.is_present(), "the code is used up");

    let audit = t.audit.entries();
    assert_eq!(audit.len(), 1);
    assert_eq!((audit[0].action, audit[0].outcome), (AuditAction::Setup.as_str(), Outcome::Success));
    assert_eq!(audit[0].client_ip, from(1).client_ip);
    assert_eq!(audit[0].request_id.as_deref(), Some("request-1"));
}

#[tokio::test]
async fn setup_requires_the_right_code() {
    let t = TestIdentity::new();
    t.use_cases.prepare_setup.execute().await.unwrap();

    let error =
        t.use_cases.complete_setup.execute(setup_input("ABCD-EFGH-JKLN", "alice", PASSWORD), &from(1)).await;

    assert!(matches!(error, Err(IdentityError::InvalidSetupCode)), "{error:?}");
    assert!(t.accounts.all().is_empty());
    assert_eq!(t.audit.entries()[0].reason, Some("wrong_setup_code"));
}

#[tokio::test]
async fn setup_without_an_issued_code_is_refused() {
    let t = TestIdentity::new();
    let error =
        t.use_cases.complete_setup.execute(setup_input(SETUP_CODE, "alice", PASSWORD), &from(1)).await;
    assert!(matches!(error, Err(IdentityError::InvalidSetupCode)), "{error:?}");
}

#[tokio::test]
async fn setup_guessing_is_throttled_per_address() {
    let t = TestIdentity::new();
    t.use_cases.prepare_setup.execute().await.unwrap();
    for _ in 0..20 {
        let _ = t
            .use_cases
            .complete_setup
            .execute(setup_input("ZZZZ-ZZZZ-ZZZZ", "alice", PASSWORD), &from(1))
            .await;
    }

    let error =
        t.use_cases.complete_setup.execute(setup_input(SETUP_CODE, "alice", PASSWORD), &from(1)).await;

    assert!(matches!(error, Err(IdentityError::TooManyAttempts { .. })), "{error:?}");
    let other =
        t.use_cases.complete_setup.execute(setup_input(SETUP_CODE, "alice", PASSWORD), &from(2)).await;
    assert!(other.is_ok(), "another address is unaffected: {other:?}");
}

#[tokio::test]
async fn setup_validates_name_and_password() {
    let t = TestIdentity::new();
    t.use_cases.prepare_setup.execute().await.unwrap();

    let bad_name =
        t.use_cases.complete_setup.execute(setup_input(SETUP_CODE, "root", PASSWORD), &from(1)).await;
    let bad_password =
        t.use_cases.complete_setup.execute(setup_input(SETUP_CODE, "alice", "short"), &from(1)).await;

    assert!(matches!(bad_name, Err(IdentityError::InvalidAccountName(_))), "{bad_name:?}");
    assert!(matches!(bad_password, Err(IdentityError::PasswordRejected(_))), "{bad_password:?}");
    assert!(t.setup_codes.is_present(), "a failed setup keeps the code");
}

#[tokio::test]
async fn setup_runs_only_once() {
    let t = TestIdentity::new();
    t.use_cases.prepare_setup.execute().await.unwrap();
    t.use_cases.complete_setup.execute(setup_input(SETUP_CODE, "alice", PASSWORD), &from(1)).await.unwrap();

    let again =
        t.use_cases.complete_setup.execute(setup_input(SETUP_CODE, "mallory", PASSWORD), &from(2)).await;

    assert!(matches!(again, Err(IdentityError::SetupCompleted)), "{again:?}");
    assert_eq!(t.accounts.all().len(), 1);
}

// Login

#[tokio::test]
async fn login_starts_a_session() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);

    let signed_in = t.use_cases.login.execute(login_input(" Alice ", PASSWORD), &from(1)).await.unwrap();

    assert_eq!(signed_in.session.account.name.as_str(), "alice", "names are matched case-insensitively");
    let sessions = t.sessions.all();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].expires_at, t.clock_now().saturating_add(Duration::from_secs(12 * 3600)));
    let audit = t.audit.entries();
    assert_eq!((audit[0].action, audit[0].outcome), (AuditAction::Login.as_str(), Outcome::Success));
}

#[tokio::test]
async fn login_failures_do_not_reveal_whether_the_account_exists() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);

    let wrong_password = t.use_cases.login.execute(login_input("alice", "wrong password"), &from(1)).await;
    let unknown = t.use_cases.login.execute(login_input("bob", PASSWORD), &from(1)).await;

    assert!(matches!(wrong_password, Err(IdentityError::InvalidCredentials)), "{wrong_password:?}");
    assert!(matches!(unknown, Err(IdentityError::InvalidCredentials)), "{unknown:?}");
    let reasons: Vec<_> = t.audit.entries().iter().map(|e| (e.actor.clone().unwrap(), e.reason)).collect();
    assert_eq!(
        reasons,
        [("alice".to_owned(), Some("wrong_password")), ("bob".to_owned(), Some("unknown_account"))]
    );
    assert!(t.sessions.all().is_empty());
}

#[tokio::test]
async fn an_account_is_locked_after_five_failures_from_any_address() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    for address in 1..=5 {
        let _ = t.use_cases.login.execute(login_input("alice", "wrong password"), &from(address)).await;
    }

    let error = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(6)).await;

    assert!(matches!(error, Err(IdentityError::TooManyAttempts { .. })), "{error:?}");
    t.clock.advance(Duration::from_secs(15 * 60));
    assert!(t.use_cases.login.execute(login_input("alice", PASSWORD), &from(6)).await.is_ok());
}

#[tokio::test]
async fn disabled_accounts_cannot_log_in() {
    let t = TestIdentity::new();
    let alice = t.add_account("alice", PASSWORD, Role::User);
    t.accounts.update(alice.id, |a| a.disabled = true);

    let error = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await;

    assert!(matches!(error, Err(IdentityError::AccountDisabled)), "{error:?}");
    assert!(t.sessions.all().is_empty());
}

// Sessions

#[tokio::test]
async fn authenticate_resolves_the_session_token() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::ReadOnly);
    let signed_in = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();

    let auth = t.use_cases.authenticate.execute(&signed_in.token).await.unwrap();

    assert_eq!(auth.session_id, signed_in.session.session_id);
    assert_eq!(auth.account.role, Role::ReadOnly);
    assert_eq!(auth.csrf_token.expose(), signed_in.session.csrf_token.expose());
    let unknown = t.use_cases.authenticate.execute(&Secret::new("token-999")).await;
    assert!(matches!(unknown, Err(IdentityError::Unauthenticated)), "{unknown:?}");
}

#[tokio::test]
async fn idle_sessions_end_and_active_ones_stay_alive() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    let signed_in = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();

    // Active every 20 minutes: each request keeps the session alive.
    for _ in 0..3 {
        t.clock.advance(Duration::from_secs(20 * 60));
        t.use_cases.authenticate.execute(&signed_in.token).await.unwrap();
    }
    // Then idle for 30 minutes.
    t.clock.advance(Duration::from_secs(30 * 60));
    let error = t.use_cases.authenticate.execute(&signed_in.token).await;

    assert!(matches!(error, Err(IdentityError::Unauthenticated)), "{error:?}");
    assert!(t.sessions.all().is_empty(), "the ended session is removed");
}

#[tokio::test]
async fn verifying_a_session_does_not_keep_it_alive() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    let signed_in = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();

    // Checked at 10 and 20 minutes: still valid, but not renewed.
    for _ in 0..2 {
        t.clock.advance(Duration::from_secs(10 * 60));
        t.use_cases.authenticate.verify(&signed_in.token).await.unwrap();
    }
    t.clock.advance(Duration::from_secs(10 * 60));
    let error = t.use_cases.authenticate.verify(&signed_in.token).await;

    assert!(matches!(error, Err(IdentityError::Unauthenticated)), "30 minutes without activity: {error:?}");
}

#[tokio::test]
async fn disabling_an_account_ends_its_sessions() {
    let t = TestIdentity::new();
    let alice = t.add_account("alice", PASSWORD, Role::User);
    let signed_in = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();
    t.accounts.update(alice.id, |a| a.disabled = true);

    let error = t.use_cases.authenticate.execute(&signed_in.token).await;

    assert!(matches!(error, Err(IdentityError::Unauthenticated)), "{error:?}");
}

#[tokio::test]
async fn logout_ends_the_session() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    let signed_in = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();

    t.use_cases.logout.execute(&signed_in.session, &from(1)).await.unwrap();

    assert!(t.use_cases.authenticate.execute(&signed_in.token).await.is_err());
    assert_eq!(t.audit.entries().last().unwrap().action, AuditAction::Logout.as_str());
}

#[tokio::test]
async fn purge_removes_ended_sessions() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();
    t.clock.advance(Duration::from_secs(10 * 60));
    t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();

    t.clock.advance(Duration::from_secs(25 * 60));
    assert_eq!(t.use_cases.purge_expired_sessions.execute().await.unwrap(), 1);
    assert_eq!(t.sessions.all().len(), 1);
}

// Password change

#[tokio::test]
async fn changing_the_password_ends_all_other_sessions() {
    let t = TestIdentity::new();
    let alice = t.add_account("alice", PASSWORD, Role::User);
    t.accounts.update(alice.id, |a| a.password_change_required = true);
    let here = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();
    let elsewhere = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(2)).await.unwrap();
    let input = ChangePasswordInput {
        current_password: Secret::new(PASSWORD),
        new_password: Secret::new("a much better passphrase"),
    };

    t.use_cases.change_password.execute(&here.session, input, &from(1)).await.unwrap();

    let account = t.accounts.all().remove(0);
    assert_eq!(account.password_hash, FakeHasher::hash_of("a much better passphrase"));
    assert!(!account.password_change_required);
    assert!(t.use_cases.authenticate.execute(&here.token).await.is_ok());
    assert!(t.use_cases.authenticate.execute(&elsewhere.token).await.is_err());
}

#[tokio::test]
async fn changing_the_password_requires_the_current_one() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    let signed_in = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();
    let input = ChangePasswordInput {
        current_password: Secret::new("not the password"),
        new_password: Secret::new("a much better passphrase"),
    };

    let error = t.use_cases.change_password.execute(&signed_in.session, input, &from(1)).await;

    assert!(matches!(error, Err(IdentityError::WrongPassword)), "{error:?}");
    assert_eq!(t.accounts.all()[0].password_hash, FakeHasher::hash_of(PASSWORD));
}

impl TestIdentity {
    fn clock_now(&self) -> zendiloos_kernel::Timestamp {
        zendiloos_kernel::Clock::now(self.clock.as_ref())
    }
}
