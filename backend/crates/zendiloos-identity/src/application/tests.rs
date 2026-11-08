use std::net::IpAddr;
use std::time::Duration;

use zendiloos_kernel::{RequestContext, Secret};

use super::IdentityError;
use super::commands::{
    AccountChanges, ChangePasswordInput, GroupChanges, LoginInput, NewAccountInput, NewGroupInput,
    PendingSetup, SetupInput,
};
use zendiloos_kernel::Outcome;

use crate::application::ports::SecurityRepository;
use crate::domain::{AddressRange, AuditAction, FailureLimit, Role, SecuritySettings};
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
    SetupInput { setup_code: Some(Secret::new(code)), name: name.to_owned(), password: Secret::new(password) }
}

/// An open setup's: no code.
fn open_input(name: &str, password: &str) -> SetupInput {
    SetupInput { setup_code: None, name: name.to_owned(), password: Secret::new(password) }
}

/// From the NAS's own network.
fn at_home(last_octet: u8) -> RequestContext {
    RequestContext { client_ip: Some(IpAddr::from([192, 168, 1, last_octet])), ..from(1) }
}

fn login_input(name: &str, password: &str) -> LoginInput {
    LoginInput { name: name.to_owned(), password: Secret::new(password) }
}

// Initial setup

#[tokio::test]
async fn prepare_setup_issues_a_code_only_while_no_admin_exists() {
    let t = TestIdentity::new();
    let PendingSetup::Code(code) = t.use_cases.prepare_setup.execute().await.unwrap() else {
        panic!("a code")
    };
    assert_eq!(code.reveal(), SETUP_CODE);
    let status = t.use_cases.setup_status.execute().await.unwrap();
    assert!(status.required && status.code_required);

    t.add_account("alice", PASSWORD, Role::Admin);

    assert!(matches!(t.use_cases.prepare_setup.execute().await.unwrap(), PendingSetup::Done));
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
    // Not blocked: the fixed limit of attempts per address applies.
    let settings = SecuritySettings { auto_block: None, ..SecuritySettings::default() };
    t.security.save_settings(&settings).await.unwrap();
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

// An open setup: installed from the ISO, no code

#[tokio::test]
async fn an_open_setup_issues_no_code_and_asks_for_none() {
    let t = TestIdentity::new();
    t.setup_codes.open();

    assert!(matches!(t.use_cases.prepare_setup.execute().await.unwrap(), PendingSetup::Open));
    assert!(!t.setup_codes.is_present(), "no code is made");
    let status = t.use_cases.setup_status.execute().await.unwrap();
    assert!(status.required && !status.code_required);

    let signed_in =
        t.use_cases.complete_setup.execute(open_input("alice", PASSWORD), &at_home(20)).await.unwrap();

    assert_eq!(signed_in.session.account.name.as_str(), "alice");
    assert_eq!(t.accounts.all()[0].role, Role::Admin);
    assert!(!t.setup_codes.is_still_open(), "the setup is closed");
    let audit = t.audit.entries();
    assert_eq!((audit[0].outcome, audit[0].reason), (Outcome::Success, Some("open_setup")));
    let status = t.use_cases.setup_status.execute().await.unwrap();
    assert!(!status.required && !status.code_required);
    let again = t.use_cases.complete_setup.execute(open_input("mallory", PASSWORD), &at_home(21)).await;
    assert!(matches!(again, Err(IdentityError::SetupCompleted)), "{again:?}");
}

#[tokio::test]
async fn an_open_setup_is_done_only_from_the_own_network() {
    let t = TestIdentity::new();
    t.setup_codes.open();
    t.use_cases.prepare_setup.execute().await.unwrap();

    // 192.0.2.0/24 is no private range.
    let elsewhere = t.use_cases.complete_setup.execute(open_input("mallory", PASSWORD), &from(7)).await;
    assert!(matches!(elsewhere, Err(IdentityError::SetupNotLocal)), "{elsewhere:?}");
    let unknown = RequestContext { client_ip: None, ..from(7) };
    let nowhere = t.use_cases.complete_setup.execute(open_input("mallory", PASSWORD), &unknown).await;
    assert!(
        matches!(nowhere, Err(IdentityError::SetupNotLocal)),
        "no address is no own network: {nowhere:?}"
    );
    assert!(t.accounts.all().is_empty());
    assert!(t.setup_codes.is_still_open());
    assert_eq!(t.audit.entries()[0].reason, Some("open_setup_from_elsewhere"));

    // A VPN's address of the shared range counts as the own network.
    let vpn = RequestContext { client_ip: Some(IpAddr::from([100, 101, 7, 8])), ..from(1) };
    assert!(t.use_cases.complete_setup.execute(open_input("alice", PASSWORD), &vpn).await.is_ok());
}

#[tokio::test]
async fn once_set_up_the_setup_is_no_longer_open() {
    let t = TestIdentity::new();
    t.setup_codes.open();
    t.add_account("alice", PASSWORD, Role::Admin);

    assert!(matches!(t.use_cases.prepare_setup.execute().await.unwrap(), PendingSetup::Done));
    assert!(!t.setup_codes.is_still_open(), "a leftover marker is removed");
}

#[tokio::test]
async fn without_an_open_setup_the_code_is_still_asked_for() {
    let t = TestIdentity::new();
    t.use_cases.prepare_setup.execute().await.unwrap();

    let without = t.use_cases.complete_setup.execute(open_input("alice", PASSWORD), &at_home(20)).await;
    assert!(matches!(without, Err(IdentityError::InvalidSetupCode)), "{without:?}");
    assert!(t.accounts.all().is_empty());
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

// Logins at the same time

type Running = Vec<tokio::task::JoinHandle<Result<super::SignedIn, IdentityError>>>;

/// Starts `logins` at once, one from each of `addresses`, with their checks
/// of the password held; returns once each is held or finished.
async fn logins_at_once(t: &TestIdentity, logins: &[(&str, &str, u8)]) -> Running {
    t.hasher.hold(logins.len());
    let running: Running = logins
        .iter()
        .map(|&(name, password, address)| {
            let login = t.use_cases.login.clone();
            let input = login_input(name, password);
            tokio::spawn(async move { login.execute(input, &from(address)).await })
        })
        .collect();
    settle(t, &running).await;
    running
}

/// Yields until every login is held at its check or finished.
async fn settle(t: &TestIdentity, running: &Running) {
    for _ in 0..10_000 {
        if running.iter().filter(|r| r.is_finished()).count() + t.hasher.waiting() == running.len() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the logins did not settle");
}

async fn outcomes(running: Running) -> Vec<Result<super::SignedIn, IdentityError>> {
    let mut outcomes = Vec::new();
    for login in running {
        outcomes.push(login.await.unwrap());
    }
    outcomes
}

#[tokio::test]
async fn logins_at_once_get_no_more_attempts_than_the_account_lock_allows() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    let guesses: Vec<_> = (1..=30).map(|address| ("alice", "wrong password", address)).collect();

    let running = logins_at_once(&t, &guesses).await;

    assert_eq!(t.hasher.waiting(), 5, "five are checked, the others refused straight away");
    t.hasher.release();
    let outcomes = outcomes(running).await;
    let refused = outcomes.iter().filter(|o| matches!(o, Err(IdentityError::TooManyAttempts { .. }))).count();
    let wrong = outcomes.iter().filter(|o| matches!(o, Err(IdentityError::InvalidCredentials))).count();
    assert_eq!((t.hasher.checks(), wrong, refused), (5, 5, 25));
    let locked = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(31)).await;
    assert!(matches!(locked, Err(IdentityError::TooManyAttempts { .. })), "{locked:?}");
}

#[tokio::test]
async fn logins_at_once_from_one_address_get_no_more_attempts_than_blocking_allows() {
    let t = TestIdentity::new();
    let guesses: Vec<_> = (0..30).map(|_| ("nobody", PASSWORD, 1)).collect();

    let running = logins_at_once(&t, &guesses).await;

    assert_eq!(t.hasher.waiting(), 10, "as many as fail before the address is blocked");
    t.hasher.release();
    outcomes(running).await;
    assert_eq!(t.hasher.checks(), 10);
    assert!(t.use_cases.check_address.is_blocked(from(1).client_ip).await.unwrap());
}

#[tokio::test]
async fn a_login_that_succeeds_frees_its_attempt_for_others() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    let mut logins = vec![("alice", PASSWORD, 5)];
    logins.extend((1..=4).map(|address| ("alice", "wrong password", address)));

    let running = logins_at_once(&t, &logins).await;
    t.hasher.release();
    let outcomes = outcomes(running).await;

    assert!(outcomes[0].is_ok(), "{:?}", outcomes[0]);
    // Four failed after it (or were cleared by it): one attempt is left.
    assert!(t.use_cases.login.execute(login_input("alice", PASSWORD), &from(6)).await.is_ok());
}

/// Alice's login with [`PASSWORD`], its check held while `change` runs.
async fn login_while<F: Future>(t: &TestIdentity, change: F) -> Result<super::SignedIn, IdentityError> {
    let running = logins_at_once(t, &[("alice", PASSWORD, 1)]).await;
    change.await;
    t.hasher.release();
    outcomes(running).await.remove(0)
}

#[tokio::test]
async fn a_login_checking_a_password_changed_meanwhile_starts_no_session() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    let here = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(2)).await.unwrap();
    let input = ChangePasswordInput {
        current_password: Secret::new(PASSWORD),
        new_password: Secret::new("a much better passphrase"),
    };

    let login = login_while(&t, async {
        t.use_cases.change_password.execute(&here.session, input, &from(2)).await.unwrap();
    })
    .await;

    assert!(matches!(login, Err(IdentityError::InvalidCredentials)), "{login:?}");
    let sessions = t.sessions.all();
    assert_eq!(sessions.len(), 1, "only the one that changed it");
    assert_eq!(sessions[0].id, here.session.session_id);
    let entry = t.audit.entries().pop().unwrap();
    assert_eq!((entry.outcome, entry.reason), (Outcome::Failure, Some("wrong_password")));
}

#[tokio::test]
async fn a_login_checking_a_password_reset_meanwhile_starts_no_session() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let alice = t.add_account("alice", PASSWORD, Role::User);

    let login = login_while(&t, async {
        let password = Secret::new("a much better passphrase");
        t.use_cases.reset_password.execute(&admin, alice.id, password, &from(9)).await.unwrap();
    })
    .await;

    assert!(matches!(login, Err(IdentityError::InvalidCredentials)), "{login:?}");
    assert!(t.sessions.all().iter().all(|s| s.account_id != alice.id));
}

#[tokio::test]
async fn a_login_for_an_account_disabled_or_deleted_meanwhile_starts_no_session() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let alice = t.add_account("alice", PASSWORD, Role::User);

    let login = login_while(&t, async {
        let changes = AccountChanges { disabled: Some(true), ..Default::default() };
        t.use_cases.update_account.execute(&admin, alice.id, changes, &from(9)).await.unwrap();
    })
    .await;
    assert!(matches!(login, Err(IdentityError::AccountDisabled)), "{login:?}");

    let changes = AccountChanges { disabled: Some(false), ..Default::default() };
    t.use_cases.update_account.execute(&admin, alice.id, changes, &from(9)).await.unwrap();
    let login = login_while(&t, async {
        t.use_cases.delete_account.execute(&admin, alice.id, &from(9)).await.unwrap();
    })
    .await;
    assert!(matches!(login, Err(IdentityError::InvalidCredentials)), "{login:?}");
    assert!(t.sessions.all().iter().all(|s| s.account_id != alice.id));
}

#[tokio::test]
async fn an_attempt_stopped_otherwise_does_not_count_as_failed() {
    let t = TestIdentity::new();
    let alice = t.add_account("alice", PASSWORD, Role::User);
    t.accounts.update(alice.id, |a| a.disabled = true);
    for address in 1..=6 {
        let disabled = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(address)).await;
        assert!(matches!(disabled, Err(IdentityError::AccountDisabled)), "{disabled:?}");
    }

    t.accounts.update(alice.id, |a| a.disabled = false);
    assert!(t.use_cases.login.execute(login_input("alice", PASSWORD), &from(7)).await.is_ok());
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
async fn a_locked_password_is_only_set_by_administrators_and_never_expires() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let input = NewAccountInput { password_locked: true, ..new_account("kiosk", Role::User) };
    let kiosk = t.use_cases.create_account.execute(&admin, input, &from(1)).await.unwrap();
    assert!(kiosk.password_locked && !kiosk.password_change_required, "nobody could follow the request");

    let policy = crate::domain::PasswordPolicy { max_age_days: Some(30), ..Default::default() };
    t.use_cases.set_password_policy.execute(&admin, policy, &from(1)).await.unwrap();
    t.clock.advance(std::time::Duration::from_secs(31 * 86_400));
    let signed_in =
        t.use_cases.login.execute(login_input("kiosk", "initial password"), &from(2)).await.unwrap();
    assert!(!t.accounts.all().iter().find(|a| a.id == kiosk.id).unwrap().password_change_required);

    let change = ChangePasswordInput {
        current_password: Secret::new("initial password"),
        new_password: Secret::new("a much better passphrase"),
    };
    let refused = t.use_cases.change_password.execute(&signed_in.session, change, &from(2)).await;
    assert!(matches!(refused, Err(IdentityError::PasswordLocked)), "{refused:?}");
}

#[tokio::test]
async fn an_old_password_must_be_changed_at_the_next_login() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let bob = t.add_account("bob", PASSWORD, Role::User);
    let policy = crate::domain::PasswordPolicy { max_age_days: Some(90), ..Default::default() };
    t.use_cases.set_password_policy.execute(&admin, policy, &from(1)).await.unwrap();

    t.clock.advance(std::time::Duration::from_secs(89 * 86_400));
    t.use_cases.login.execute(login_input("bob", PASSWORD), &from(2)).await.unwrap();
    assert!(!t.accounts.all().iter().find(|a| a.id == bob.id).unwrap().password_change_required);
    t.clock.advance(std::time::Duration::from_secs(2 * 86_400));
    t.use_cases.login.execute(login_input("bob", PASSWORD), &from(2)).await.unwrap();
    assert!(t.accounts.all().iter().find(|a| a.id == bob.id).unwrap().password_change_required);
}

#[tokio::test]
async fn only_administrators_set_the_password_policy_and_it_applies_to_new_passwords() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let bob = signed_in(&t, "bob", Role::User).await;
    let strict = crate::domain::PasswordPolicy { min_length: 16, digit: true, ..Default::default() };

    let refused = t.use_cases.set_password_policy.execute(&bob, strict, &from(1)).await;
    assert!(matches!(refused, Err(IdentityError::Forbidden)), "{refused:?}");
    let too_lax = crate::domain::PasswordPolicy { min_length: 8, ..Default::default() };
    let refused = t.use_cases.set_password_policy.execute(&admin, too_lax, &from(1)).await;
    assert!(matches!(refused, Err(IdentityError::InvalidPolicy(_))), "{refused:?}");
    t.use_cases.set_password_policy.execute(&admin, strict, &from(1)).await.unwrap();
    assert_eq!(t.use_cases.password_policy.execute(&bob).await.unwrap(), strict);

    let input =
        NewAccountInput { password: Secret::new("sixteen characters"), ..new_account("carol", Role::User) };
    let rejected = t.use_cases.create_account.execute(&admin, input, &from(1)).await;
    assert!(matches!(rejected, Err(IdentityError::PasswordRejected(_))), "no digit: {rejected:?}");
}

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

// Accounts and groups, managed by administrators

async fn signed_in(t: &TestIdentity, name: &str, role: Role) -> super::AuthContext {
    t.add_account(name, PASSWORD, role);
    t.use_cases.login.execute(login_input(name, PASSWORD), &from(9)).await.unwrap().session
}

fn new_account(name: &str, role: Role) -> NewAccountInput {
    NewAccountInput {
        name: name.to_owned(),
        full_name: Some(" Bob Builder ".to_owned()),
        description: None,
        email: None,
        role,
        password: Secret::new("initial password"),
        password_locked: false,
    }
}

#[tokio::test]
async fn setup_creates_the_linux_user_with_samba_password() {
    let t = TestIdentity::new();
    t.use_cases.prepare_setup.execute().await.unwrap();
    t.use_cases.complete_setup.execute(setup_input(SETUP_CODE, "alice", PASSWORD), &from(1)).await.unwrap();
    assert_eq!(t.system.calls(), ["create_user alice", "set_password alice"]);
}

#[tokio::test]
async fn setup_refuses_names_the_system_uses() {
    let t = TestIdentity::new();
    t.system.taken.lock().unwrap().push("debian".to_owned());
    t.use_cases.prepare_setup.execute().await.unwrap();

    let result =
        t.use_cases.complete_setup.execute(setup_input(SETUP_CODE, "debian", PASSWORD), &from(1)).await;

    assert!(matches!(result, Err(IdentityError::NameTaken)), "{result:?}");
    assert!(t.accounts.all().is_empty());
    assert_eq!(t.system.calls(), ["create_user debian"]);
}

#[tokio::test]
async fn changing_the_password_also_changes_it_for_smb() {
    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::User).await;
    let input = ChangePasswordInput {
        current_password: Secret::new(PASSWORD),
        new_password: Secret::new("a brand new passphrase"),
    };
    t.use_cases.change_password.execute(&alice, input, &from(1)).await.unwrap();
    assert_eq!(t.system.calls(), ["set_password alice"]);
}

#[tokio::test]
async fn accounts_keep_a_description_and_an_email_address() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let input = NewAccountInput {
        description: Some(" Laptop in the kitchen ".into()),
        email: Some("bob@example.org".into()),
        ..new_account("bob", Role::User)
    };
    let bob = t.use_cases.create_account.execute(&admin, input, &from(1)).await.unwrap();
    assert_eq!(bob.description.as_deref(), Some("Laptop in the kitchen"));

    let changes = AccountChanges { email: Some(Some("not an address".into())), ..Default::default() };
    let refused = t.use_cases.update_account.execute(&admin, bob.id, changes, &from(1)).await;
    assert!(matches!(refused, Err(IdentityError::InvalidEmail(_))), "{refused:?}");
    let changes = AccountChanges { description: Some(None), email: Some(None), ..Default::default() };
    let bob = t.use_cases.update_account.execute(&admin, bob.id, changes, &from(1)).await.unwrap();
    assert_eq!((bob.description, bob.email), (None, None));
}

#[tokio::test]
async fn admins_create_accounts_that_must_change_their_password() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;

    let bob =
        t.use_cases.create_account.execute(&admin, new_account("bob", Role::User), &from(1)).await.unwrap();

    assert_eq!(bob.full_name.as_deref(), Some("Bob Builder"));
    assert!(bob.password_change_required);
    assert_eq!(bob.password_hash, FakeHasher::hash_of("initial password"));
    assert_eq!(t.system.calls(), ["create_user bob", "set_password bob"]);
    let entry = t.audit.entries().pop().unwrap();
    assert_eq!(entry.action, AuditAction::AccountCreate.as_str());
    assert_eq!((entry.actor.as_deref(), entry.target.as_deref()), (Some("alice"), Some("bob")));
}

#[tokio::test]
async fn account_names_must_be_free_in_zendiloos_and_on_the_host() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    t.system.taken.lock().unwrap().push("postfix".to_owned());

    for name in ["alice", "postfix"] {
        let result =
            t.use_cases.create_account.execute(&admin, new_account(name, Role::User), &from(1)).await;
        assert!(matches!(result, Err(IdentityError::NameTaken)), "{name}: {result:?}");
    }
    assert_eq!(t.accounts.all().len(), 1);
}

#[tokio::test]
async fn a_failed_creation_leaves_no_linux_user_behind() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    t.system.fail_passwords.store(true, std::sync::atomic::Ordering::Relaxed);

    let result = t.use_cases.create_account.execute(&admin, new_account("bob", Role::User), &from(1)).await;

    assert!(matches!(result, Err(IdentityError::Port(_))), "{result:?}");
    assert_eq!(t.system.calls(), ["create_user bob", "set_password bob", "delete_user bob"]);
    assert_eq!(t.accounts.all().len(), 1);
}

#[tokio::test]
async fn only_admins_manage_accounts() {
    let t = TestIdentity::new();
    let user = signed_in(&t, "carol", Role::ReadOnly).await;
    let result = t.use_cases.create_account.execute(&user, new_account("bob", Role::Admin), &from(1)).await;
    assert!(matches!(result, Err(IdentityError::Forbidden)), "{result:?}");
    assert!(matches!(t.use_cases.list_accounts.execute(&user).await, Err(IdentityError::Forbidden)));
    assert!(t.system.calls().is_empty());
}

#[tokio::test]
async fn disabling_an_account_ends_its_sessions_and_smb_access() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let bob = signed_in(&t, "bob", Role::User).await;
    let changes = AccountChanges { disabled: Some(true), ..AccountChanges::default() };

    let updated =
        t.use_cases.update_account.execute(&admin, bob.account.id, changes, &from(1)).await.unwrap();

    assert!(updated.disabled);
    assert_eq!(t.system.calls(), ["set_enabled bob false"]);
    assert!(t.sessions.all().iter().all(|s| s.account_id != bob.account.id));
}

#[tokio::test]
async fn admins_cannot_lock_themselves_out() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let id = admin.account.id;
    let request = from(1);
    let update = |changes| t.use_cases.update_account.execute(&admin, id, changes, &request);

    let demote = AccountChanges { role: Some(Role::User), ..AccountChanges::default() };
    assert!(matches!(update(demote).await, Err(IdentityError::OwnAccount)));
    let disable = AccountChanges { disabled: Some(true), ..AccountChanges::default() };
    assert!(matches!(update(disable).await, Err(IdentityError::OwnAccount)));
    let rename = AccountChanges { full_name: Some(Some("Alice A.".to_owned())), ..AccountChanges::default() };
    assert_eq!(update(rename).await.unwrap().full_name.as_deref(), Some("Alice A."));

    let delete = t.use_cases.delete_account.execute(&admin, id, &from(1)).await;
    assert!(matches!(delete, Err(IdentityError::OwnAccount)));
    let reset =
        t.use_cases.reset_password.execute(&admin, id, Secret::new("another passphrase"), &from(1)).await;
    assert!(matches!(reset, Err(IdentityError::OwnAccount)));
}

#[tokio::test]
async fn the_last_enabled_admin_stays() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let carol = t.add_account("carol", PASSWORD, Role::Admin);
    // A context that outlived its account, so that carol is the last admin.
    let mut stale = admin.clone();
    stale.account.id = crate::domain::AccountId::new(99);
    let alice_id = admin.account.id;
    let disable = AccountChanges { disabled: Some(true), ..AccountChanges::default() };
    t.use_cases.update_account.execute(&stale, alice_id, disable, &from(1)).await.unwrap();

    let result = t.use_cases.delete_account.execute(&stale, carol.id, &from(1)).await;
    assert!(matches!(result, Err(IdentityError::LastAdmin)), "{result:?}");
    let demote = AccountChanges { role: Some(Role::User), ..AccountChanges::default() };
    let result = t.use_cases.update_account.execute(&stale, carol.id, demote, &from(1)).await;
    assert!(matches!(result, Err(IdentityError::LastAdmin)), "{result:?}");
}

#[tokio::test]
async fn resetting_a_password_requires_a_change_and_ends_sessions() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let bob = signed_in(&t, "bob", Role::User).await;

    t.use_cases
        .reset_password
        .execute(&admin, bob.account.id, Secret::new("temporary passphrase"), &from(1))
        .await
        .unwrap();

    let stored = t.accounts.all().into_iter().find(|a| a.id == bob.account.id).unwrap();
    assert!(stored.password_change_required);
    assert_eq!(stored.password_hash, FakeHasher::hash_of("temporary passphrase"));
    assert_eq!(t.system.calls(), ["set_password bob"]);
    assert!(t.sessions.all().iter().all(|s| s.account_id != bob.account.id));
}

#[tokio::test]
async fn deleting_an_account_removes_its_linux_user_and_memberships() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let bob = t.add_account("bob", PASSWORD, Role::User);
    let input =
        NewGroupInput { name: "family".into(), description: None, members: vec![bob.id, admin.account.id] };
    let family = t.use_cases.create_group.execute(&admin, input, &from(1)).await.unwrap();

    t.use_cases.delete_account.execute(&admin, bob.id, &from(1)).await.unwrap();

    assert!(t.accounts.all().iter().all(|a| a.id != bob.id));
    assert_eq!(t.system.calls().last().unwrap(), "delete_user bob");
    assert_eq!(t.groups.all()[0].members, [admin.account.id]);
    assert_eq!(family.members, [admin.account.id, bob.id]);
}

#[tokio::test]
async fn groups_are_linux_groups_with_the_same_members() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let bob = t.add_account("bob", PASSWORD, Role::User);
    let input = NewGroupInput {
        name: "family".into(),
        description: Some("Everyone at home".into()),
        members: vec![bob.id, admin.account.id, bob.id],
    };

    let family = t.use_cases.create_group.execute(&admin, input, &from(1)).await.unwrap();
    assert_eq!(family.members, [admin.account.id, bob.id], "sorted, without duplicates");

    let changes = GroupChanges { members: Some(vec![bob.id]), description: Some(None) };
    let family = t.use_cases.update_group.execute(&admin, family.id, changes, &from(1)).await.unwrap();
    assert_eq!(family.members, [bob.id]);
    assert_eq!(family.description, None);

    t.use_cases.delete_group.execute(&admin, family.id, &from(1)).await.unwrap();
    assert!(t.groups.all().is_empty());
    assert_eq!(
        t.system.calls(),
        [
            "create_group family",
            "set_group_members family [alice,bob]",
            "set_group_members family [bob]",
            "delete_group family",
        ]
    );
}

#[tokio::test]
async fn others_hear_of_new_members_and_of_a_group_before_it_goes() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    let bob = t.add_account("bob", PASSWORD, Role::User);
    let input = NewGroupInput { name: "kids".into(), description: None, members: vec![bob.id] };
    let kids = t.use_cases.create_group.execute(&admin, input, &from(1)).await.unwrap();

    let same = GroupChanges { members: Some(vec![bob.id]), description: None };
    t.use_cases.update_group.execute(&admin, kids.id, same, &from(1)).await.unwrap();
    let more = GroupChanges { members: Some(vec![bob.id, admin.account.id]), description: None };
    t.use_cases.update_group.execute(&admin, kids.id, more, &from(1)).await.unwrap();

    t.group_observer.refuse.store(true, std::sync::atomic::Ordering::Relaxed);
    let refused = t.use_cases.delete_group.execute(&admin, kids.id, &from(1)).await;
    assert!(refused.is_err(), "the group stays while shares cannot let go of it");
    assert_eq!(t.groups.all().len(), 1);
    t.group_observer.refuse.store(false, std::sync::atomic::Ordering::Relaxed);
    t.use_cases.delete_group.execute(&admin, kids.id, &from(1)).await.unwrap();

    let id = kids.id.get();
    let events = t.group_observer.events.lock().unwrap().clone();
    assert_eq!(events, [format!("members {id} [{}]", bob.id.get()), format!("deleting {id}")]);
}

#[tokio::test]
async fn groups_need_free_names_and_existing_members() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "alice", Role::Admin).await;
    t.system.taken.lock().unwrap().push("sudo".to_owned());

    let input = |name: &str, members| NewGroupInput { name: name.into(), description: None, members };
    let taken = t.use_cases.create_group.execute(&admin, input("sudo", vec![]), &from(1)).await;
    assert!(matches!(taken, Err(IdentityError::NameTaken)), "{taken:?}");
    let unknown = crate::domain::AccountId::new(42);
    let missing = t.use_cases.create_group.execute(&admin, input("media", vec![unknown]), &from(1)).await;
    assert!(matches!(missing, Err(IdentityError::AccountNotFound)), "{missing:?}");
    assert_eq!(t.system.calls(), ["create_group sudo"]);
}

// Preferences

#[tokio::test]
async fn each_account_keeps_its_own_preferences() {
    use crate::domain::{Language, Preferences, Theme};

    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::ReadOnly).await;
    let bob = signed_in(&t, "bob", Role::User).await;
    assert_eq!(t.use_cases.preferences.execute(&alice).await.unwrap(), Preferences::default());

    let chosen =
        Preferences { language: Some(Language::German), theme: Some(Theme::Dark), ..Preferences::default() };
    t.use_cases.save_preferences.execute(&alice, chosen.clone()).await.unwrap();

    assert_eq!(t.use_cases.preferences.execute(&alice).await.unwrap(), chosen);
    assert_eq!(t.use_cases.preferences.execute(&bob).await.unwrap(), Preferences::default());
}

// Security: idle time, account lock, blocked addresses, sessions

#[tokio::test]
async fn ipv6_addresses_of_one_network_count_as_one_client() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    let at = |host: u16| RequestContext {
        client_ip: Some(IpAddr::from([0x2001, 0xdb8, 0, 1, 0, 0, 0, host])),
        ..from(1)
    };
    // Each attempt from another address of the same /64.
    for host in 0..10 {
        let _ = t.use_cases.login.execute(login_input("nobody", PASSWORD), &at(host)).await;
    }

    let blocked = t.use_cases.login.execute(login_input("alice", PASSWORD), &at(77)).await;

    assert!(matches!(blocked, Err(IdentityError::AddressBlocked)), "{blocked:?}");
    let entry = t.audit.entries().into_iter().find(|e| e.action == "identity.address_block").unwrap();
    assert_eq!(entry.target.as_deref(), Some("2001:db8:0:1::/64"));
    let neighbour =
        RequestContext { client_ip: Some(IpAddr::from([0x2001, 0xdb8, 0, 2, 0, 0, 0, 1])), ..from(1) };
    assert!(
        t.use_cases.login.execute(login_input("alice", PASSWORD), &neighbour).await.is_ok(),
        "the next /64 is another"
    );
}

#[tokio::test]
async fn an_address_that_fails_too_often_is_blocked_for_a_day() {
    let t = TestIdentity::new();
    t.add_account("alice", PASSWORD, Role::User);
    for _ in 0..10 {
        let _ = t.use_cases.login.execute(login_input("nobody", PASSWORD), &from(1)).await;
    }

    let blocked = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await;

    assert!(matches!(blocked, Err(IdentityError::AddressBlocked)), "{blocked:?}");
    assert!(t.use_cases.check_address.is_blocked(from(1).client_ip).await.unwrap());
    assert!(t.use_cases.login.execute(login_input("alice", PASSWORD), &from(2)).await.is_ok());
    let entry = t.audit.entries().into_iter().find(|e| e.action == "identity.address_block").unwrap();
    assert_eq!((entry.target.as_deref(), entry.reason), (Some("192.0.2.1"), Some("automatic")));
    t.clock.advance(Duration::from_secs(86_400));
    assert!(!t.use_cases.check_address.is_blocked(from(1).client_ip).await.unwrap(), "lifted after a day");
    t.use_cases.purge_expired_sessions.execute().await.unwrap();
    assert!(t.security.blocked().is_empty(), "and removed");
}

#[tokio::test]
async fn allowed_addresses_are_never_blocked() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let settings = SecuritySettings {
        allowed: vec![AddressRange::parse("192.0.2.0/28").unwrap()],
        ..SecuritySettings::default()
    };
    t.use_cases.set_security.execute(&admin, settings, &from(9)).await.unwrap();
    for _ in 0..12 {
        let _ = t.use_cases.login.execute(login_input("nobody", PASSWORD), &from(1)).await;
    }

    assert!(!t.use_cases.check_address.is_blocked(from(1).client_ip).await.unwrap());
    assert!(t.security.blocked().is_empty());
}

#[tokio::test]
async fn accounts_lock_as_set_or_not_at_all() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    t.add_account("alice", PASSWORD, Role::User);
    let set =
        |account_lock| SecuritySettings { account_lock, auto_block: None, ..SecuritySettings::default() };
    let three = Some(FailureLimit { failures: 3, window_minutes: 1 });
    t.use_cases.set_security.execute(&admin, set(three), &from(9)).await.unwrap();
    for address in 1..=3 {
        let _ = t.use_cases.login.execute(login_input("alice", "wrong password"), &from(address)).await;
    }

    let locked = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(4)).await;

    assert!(matches!(locked, Err(IdentityError::TooManyAttempts { .. })), "{locked:?}");
    t.clock.advance(Duration::from_secs(60));
    assert!(t.use_cases.login.execute(login_input("alice", PASSWORD), &from(4)).await.is_ok());

    t.use_cases.set_security.execute(&admin, set(None), &from(9)).await.unwrap();
    for address in 1..=6 {
        let _ = t.use_cases.login.execute(login_input("alice", "wrong password"), &from(address)).await;
    }
    assert!(t.use_cases.login.execute(login_input("alice", PASSWORD), &from(7)).await.is_ok(), "never locks");
}

#[tokio::test]
async fn sessions_end_after_the_idle_time_set() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let settings = SecuritySettings { idle_minutes: 5, ..SecuritySettings::default() };
    let stored = t.use_cases.set_security.execute(&admin, settings, &from(9)).await.unwrap();
    assert_eq!(stored.idle_minutes, 5);
    t.add_account("alice", PASSWORD, Role::User);
    let signed_in = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(1)).await.unwrap();

    t.clock.advance(Duration::from_secs(5 * 60));

    let ended = t.use_cases.authenticate.execute(&signed_in.token).await;
    assert!(matches!(ended, Err(IdentityError::Unauthenticated)), "{ended:?}");
    let bad = SecuritySettings { idle_minutes: 1, ..SecuritySettings::default() };
    let refused = t.use_cases.set_security.execute(&admin, bad, &from(9)).await;
    assert!(matches!(refused, Err(IdentityError::InvalidSecurity(_))), "{refused:?}");
}

#[tokio::test]
async fn admins_block_and_lift_by_hand_but_never_their_own_address() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let user = signed_in(&t, "bob", Role::User).await;
    let me = from(9);
    let block = |range: &'static str| t.use_cases.block_address.execute(&admin, range, &me);

    assert!(matches!(block("192.0.2.0/24").await, Err(IdentityError::OwnAddress)));
    assert!(matches!(block("localhost").await, Err(IdentityError::InvalidAddressRange(_))));
    assert!(matches!(block("127.0.0.1").await, Err(IdentityError::AddressAllowed)));
    let blocked = block("198.51.100.7/24").await.unwrap();
    assert_eq!(
        (blocked.range.to_string(), blocked.until, blocked.automatic),
        ("198.51.100.0/24".into(), None, false)
    );
    let list = t.use_cases.blocked_addresses.execute(&admin).await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(t.use_cases.check_address.is_blocked(Some(IpAddr::from([198, 51, 100, 20]))).await.unwrap());
    let forbidden = t.use_cases.block_address.execute(&user, "203.0.113.1", &from(9)).await;
    assert!(matches!(forbidden, Err(IdentityError::Forbidden)), "{forbidden:?}");

    t.use_cases.unblock_address.execute(&admin, "198.51.100.0/24", &from(9)).await.unwrap();
    assert!(!t.use_cases.check_address.is_blocked(Some(IpAddr::from([198, 51, 100, 20]))).await.unwrap());
    let again = t.use_cases.unblock_address.execute(&admin, "198.51.100.0/24", &from(9)).await;
    assert!(matches!(again, Err(IdentityError::BlockNotFound)), "{again:?}");
}

#[tokio::test]
async fn admins_see_every_session_and_end_one() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    t.add_account("bob", PASSWORD, Role::User);
    t.clock.advance(Duration::from_secs(5));
    let bob = t.use_cases.login.execute(login_input("bob", PASSWORD), &from(3)).await.unwrap();

    let sessions = t.use_cases.sessions.execute(&admin).await.unwrap();

    let seen: Vec<_> = sessions.iter().map(|s| (s.account.as_str(), s.client_ip, s.current)).collect();
    assert_eq!(seen, [("bob", from(3).client_ip, false), ("admin", from(9).client_ip, true)]);
    t.use_cases.end_session.execute(&admin, bob.session.session_id, &from(9)).await.unwrap();
    let ended = t.use_cases.authenticate.execute(&bob.token).await;
    assert!(matches!(ended, Err(IdentityError::Unauthenticated)), "{ended:?}");
    let entry = t.audit.entries().last().cloned().unwrap();
    assert_eq!((entry.action, entry.target.as_deref()), ("identity.session_end", Some("bob")));
    let bob_again = t.use_cases.login.execute(login_input("bob", PASSWORD), &from(3)).await.unwrap();
    let forbidden = t.use_cases.sessions.execute(&bob_again.session).await;
    assert!(matches!(forbidden, Err(IdentityError::Forbidden)), "{forbidden:?}");
}
