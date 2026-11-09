//! Two-factor sign-in: setting it up, the second step of a sign-in, turning
//! it off and resetting it.

use std::time::Duration;

use zendiloos_kernel::{Clock, Outcome, RequestContext, Secret};

use super::{PASSWORD, from, login_input, password_login, signed_in};
use crate::application::commands::ChangePasswordInput;
use crate::application::{AuthContext, IdentityError, LoginOutcome, SecondFactorChallenge, SignedIn};
use crate::domain::{AccountId, AuditAction, Role, TotpSecret};
use crate::testing::TestIdentity;

/// Sets up the second factor of `auth`'s account and moves the clock on to
/// the next time step, whose code is not taken yet; gives the recovery codes.
async fn turn_on(t: &TestIdentity, auth: &AuthContext) -> Vec<String> {
    t.use_cases.begin_two_factor.execute(auth, &Secret::new(PASSWORD), &from(9)).await.unwrap();
    let code = Secret::new(t.code_now(auth.account.id, 0));
    let codes = t.use_cases.confirm_two_factor.execute(auth, &code, &from(9)).await.unwrap();
    t.clock.advance(Duration::from_secs(30));
    codes
}

/// Alice, signed in, with her second factor on; and her recovery codes.
async fn alice_with_two_factor(t: &TestIdentity) -> (AuthContext, Vec<String>) {
    let alice = signed_in(t, "alice", Role::User).await;
    let codes = turn_on(t, &alice).await;
    (alice, codes)
}

/// The first step of Alice's sign-in from `request`: the password.
async fn password_step(t: &TestIdentity, request: &RequestContext) -> SecondFactorChallenge {
    match t.use_cases.login.execute(login_input("alice", PASSWORD), request).await.unwrap() {
        LoginOutcome::SecondFactor(challenge) => challenge,
        LoginOutcome::SignedIn(_) => panic!("a second factor was asked for"),
    }
}

async fn code_step(
    t: &TestIdentity,
    challenge: &SecondFactorChallenge,
    code: &str,
    request: &RequestContext,
) -> Result<SignedIn, IdentityError> {
    t.use_cases.complete_sign_in.execute(&challenge.token, &Secret::new(code), request).await
}

/// Another session of `name`'s account.
async fn signed_in_again(t: &TestIdentity, name: &str) -> AuthContext {
    password_login(&t.use_cases.login, login_input(name, PASSWORD), &from(9)).await.unwrap().session
}

async fn confirm_setup(
    t: &TestIdentity,
    auth: &AuthContext,
    code: Secret,
) -> Result<Vec<String>, IdentityError> {
    t.use_cases.confirm_two_factor.execute(auth, &code, &from(9)).await
}

fn last_audit(t: &TestIdentity) -> (&'static str, Outcome, Option<&'static str>) {
    let entry = t.audit.entries().pop().unwrap();
    (entry.action, entry.outcome, entry.reason)
}

// Setting it up

#[tokio::test]
async fn setting_up_a_second_factor_takes_its_first_code() {
    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::User).await;
    let elsewhere =
        password_login(&t.use_cases.login, login_input("alice", PASSWORD), &from(2)).await.unwrap();

    let setup = t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await.unwrap();

    let secret = TotpSecret::from_base32(setup.secret.expose()).expect("base32");
    assert_eq!(t.two_factor.get(alice.account.id).unwrap().pending, Some(secret.clone()));
    assert_eq!(
        setup.uri.expose(),
        format!(
            "otpauth://totp/zendiloOS:alice?secret={}&issuer=zendiloOS&algorithm=SHA1&digits=6&period=30",
            setup.secret.expose()
        )
    );
    let status = t.use_cases.two_factor.execute(&alice).await.unwrap();
    assert!(!status.on, "not before its first code");

    let wrong = Secret::new("000000");
    let refused = t.use_cases.confirm_two_factor.execute(&alice, &wrong, &from(9)).await;
    assert!(matches!(refused, Err(IdentityError::InvalidCode)), "{refused:?}");

    let code = Secret::new(t.code_now(alice.account.id, 0));
    let codes = t.use_cases.confirm_two_factor.execute(&alice, &code, &from(9)).await.unwrap();

    assert_eq!(codes.len(), 10);
    assert!(codes.iter().all(|code| code.len() == 11 && code.as_bytes()[5] == b'-'), "{codes:?}");
    let status = t.use_cases.two_factor.execute(&alice).await.unwrap();
    assert!(status.on);
    assert_eq!(status.recovery_codes_left, 10);
    let stored = t.two_factor.get(alice.account.id).unwrap();
    assert!(stored.recovery.iter().all(|hash| !codes.iter().any(|code| hash.as_str().contains(code))));
    let sessions = t.sessions.all();
    assert_eq!(sessions.len(), 1, "the other session ended");
    assert_eq!(sessions[0].id, alice.session_id);
    assert_ne!(sessions[0].id, elsewhere.session.session_id);
    assert_eq!(last_audit(&t), (AuditAction::TwoFactorEnable.as_str(), Outcome::Success, None));

    let again = t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await;
    assert!(matches!(again, Err(IdentityError::TwoFactorOn)), "{again:?}");
}

#[tokio::test]
async fn a_setup_begun_again_takes_the_new_secret_only() {
    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::User).await;
    t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await.unwrap();
    let first = Secret::new(t.code_now(alice.account.id, 0));

    t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await.unwrap();

    let refused = t.use_cases.confirm_two_factor.execute(&alice, &first, &from(9)).await;
    assert!(matches!(refused, Err(IdentityError::InvalidCode)), "{refused:?}");
    let code = Secret::new(t.code_now(alice.account.id, 0));
    assert!(t.use_cases.confirm_two_factor.execute(&alice, &code, &from(9)).await.is_ok());
}

#[tokio::test]
async fn setting_up_takes_the_password_and_wrong_ones_count() {
    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::User).await;
    let wrong = Secret::new("wrong password");

    let refused = t.use_cases.begin_two_factor.execute(&alice, &wrong, &from(9)).await;

    assert!(matches!(refused, Err(IdentityError::WrongPassword)), "{refused:?}");
    assert!(t.two_factor.get(alice.account.id).is_none(), "no secret is made");
    assert_eq!(
        last_audit(&t),
        (AuditAction::TwoFactorEnable.as_str(), Outcome::Failure, Some("wrong_password"))
    );
    for _ in 0..4 {
        let _ = t.use_cases.begin_two_factor.execute(&alice, &wrong, &from(9)).await;
    }
    let locked = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(2)).await;
    assert!(matches!(locked, Err(IdentityError::TooManyAttempts { .. })), "{locked:?}");
}

#[tokio::test]
async fn a_setup_whose_password_changed_meanwhile_makes_no_secret() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let alice = signed_in(&t, "alice", Role::User).await;
    t.hasher.hold(1);
    let begin = t.use_cases.begin_two_factor.clone();
    let (auth, request) = (alice.clone(), from(9));
    let running = tokio::spawn(async move { begin.execute(&auth, &Secret::new(PASSWORD), &request).await });
    while t.hasher.waiting() == 0 {
        tokio::task::yield_now().await;
    }

    let password = Secret::new("a much better passphrase");
    t.use_cases.reset_password.execute(&admin, alice.account.id, password, &from(9)).await.unwrap();
    t.hasher.release();

    let refused = running.await.unwrap();
    assert!(matches!(refused, Err(IdentityError::Unauthenticated)), "{refused:?}");
    assert!(t.two_factor.get(alice.account.id).is_none());
}

#[tokio::test]
async fn confirming_needs_a_setup_begun() {
    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::User).await;

    let refused = t.use_cases.confirm_two_factor.execute(&alice, &Secret::new("123456"), &from(9)).await;

    assert!(matches!(refused, Err(IdentityError::NoTwoFactorSetup)), "{refused:?}");
}

/// A setup waits for its first code from the session that began it, for
/// ten minutes, and takes three wrong codes: a session taken over cannot
/// guess the code of a setup its owner left, nor make one last.
#[tokio::test]
async fn a_setup_is_confirmed_by_the_session_that_began_it_while_it_waits() {
    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::User).await;
    let other = signed_in_again(&t, "alice").await;
    t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await.unwrap();
    let code = || Secret::new(t.code_now(alice.account.id, 0));

    let elsewhere = confirm_setup(&t, &other, code()).await;
    assert!(matches!(elsewhere, Err(IdentityError::NoTwoFactorSetup)), "{elsewhere:?}");
    t.clock.advance(Duration::from_secs(10 * 60));
    let late = confirm_setup(&t, &alice, code()).await;
    assert!(matches!(late, Err(IdentityError::NoTwoFactorSetup)), "{late:?}");

    // Begun again: three wrong codes end it, the last one says so.
    t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await.unwrap();
    for _ in 0..2 {
        let wrong = confirm_setup(&t, &alice, Secret::new("000000")).await;
        assert!(matches!(wrong, Err(IdentityError::InvalidCode)), "{wrong:?}");
    }
    assert_eq!(last_audit(&t), (AuditAction::TwoFactorEnable.as_str(), Outcome::Failure, Some("wrong_code")));
    let third = confirm_setup(&t, &alice, Secret::new("000000")).await;
    assert!(matches!(third, Err(IdentityError::NoTwoFactorSetup)), "{third:?}");
    assert_eq!(t.two_factor.get(alice.account.id).unwrap().pending, None, "its secret is gone");
    let after = confirm_setup(&t, &alice, Secret::new("123456")).await;
    assert!(matches!(after, Err(IdentityError::NoTwoFactorSetup)), "{after:?}");

    // Begun once more, it is confirmed as before.
    t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await.unwrap();
    assert_eq!(confirm_setup(&t, &alice, code()).await.unwrap().len(), 10);
}

/// A new password ends a setup begun with the old one.
#[tokio::test]
async fn a_setup_ends_when_the_password_changes() {
    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::User).await;
    t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await.unwrap();
    let input = ChangePasswordInput {
        current_password: Secret::new(PASSWORD),
        new_password: Secret::new("a different horse battery"),
    };
    t.use_cases.change_password.execute(&alice, input, &from(9)).await.unwrap();

    let code = Secret::new(t.code_now(alice.account.id, 0));
    let refused = t.use_cases.confirm_two_factor.execute(&alice, &code, &from(9)).await;
    assert!(matches!(refused, Err(IdentityError::NoTwoFactorSetup)), "{refused:?}");
}

/// An account whose password an administrator sets may be shared: one of
/// those sharing it cannot lock the others out with a second factor.
#[tokio::test]
async fn an_account_whose_password_an_administrator_sets_takes_no_second_factor() {
    let t = TestIdentity::new();
    let account = t.add_account("kiosk", PASSWORD, Role::User);
    t.accounts.update(account.id, |a| a.password_locked = true);
    let kiosk = signed_in_again(&t, "kiosk").await;

    let refused = t.use_cases.begin_two_factor.execute(&kiosk, &Secret::new(PASSWORD), &from(9)).await;
    assert!(matches!(refused, Err(IdentityError::PasswordLocked)), "{refused:?}");
    assert!(t.two_factor.get(kiosk.account.id).is_none());
}

#[tokio::test]
async fn codes_typed_wrong_while_setting_up_do_not_lock_the_account() {
    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::User).await;
    t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await.unwrap();

    for _ in 0..10 {
        let _ = t.use_cases.confirm_two_factor.execute(&alice, &Secret::new("000000"), &from(9)).await;
    }

    assert!(password_login(&t.use_cases.login, login_input("alice", PASSWORD), &from(9)).await.is_ok());
}

#[tokio::test]
async fn a_setup_not_confirmed_leaves_the_password_alone_enough() {
    let t = TestIdentity::new();
    let alice = signed_in(&t, "alice", Role::User).await;
    t.use_cases.begin_two_factor.execute(&alice, &Secret::new(PASSWORD), &from(9)).await.unwrap();

    assert!(password_login(&t.use_cases.login, login_input("alice", PASSWORD), &from(2)).await.is_ok());
}

// Signing in

#[tokio::test]
async fn with_a_second_factor_the_password_alone_starts_no_session() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    let sessions_before = t.sessions.all().len();

    let challenge = password_step(&t, &from(2)).await;

    assert_eq!(t.sessions.all().len(), sessions_before, "no session yet");
    assert_eq!(challenge.expires_at, t.clock.now().saturating_add(Duration::from_secs(5 * 60)));
    let signed_in = code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await.unwrap();
    assert_eq!(signed_in.session.account.name.as_str(), "alice");
    assert_eq!(t.sessions.all().len(), sessions_before + 1);
    assert_eq!(last_audit(&t), (AuditAction::Login.as_str(), Outcome::Success, Some("second_factor")));
}

#[tokio::test]
async fn a_code_of_the_step_before_or_after_is_taken_too() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    t.clock.advance(Duration::from_secs(60));

    let challenge = password_step(&t, &from(2)).await;
    assert!(code_step(&t, &challenge, &t.code_now(alice.account.id, -1), &from(2)).await.is_ok());
    let challenge = password_step(&t, &from(2)).await;
    assert!(code_step(&t, &challenge, &t.code_now(alice.account.id, 1), &from(2)).await.is_ok());

    let challenge = password_step(&t, &from(2)).await;
    let too_far = code_step(&t, &challenge, &t.code_now(alice.account.id, 3), &from(2)).await;
    assert!(matches!(too_far, Err(IdentityError::InvalidCode)), "{too_far:?}");
}

#[tokio::test]
async fn a_code_is_taken_once() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    let code = t.code_now(alice.account.id, 0);
    let first = password_step(&t, &from(2)).await;
    let second = password_step(&t, &from(2)).await;

    let here = from(2);
    let (a, b) = tokio::join!(code_step(&t, &first, &code, &here), code_step(&t, &second, &code, &here));

    let taken = [&a, &b].iter().filter(|outcome| outcome.is_ok()).count();
    assert_eq!(taken, 1, "{a:?} {b:?}");
    assert!([&a, &b].iter().any(|outcome| matches!(outcome, Err(IdentityError::InvalidCode))));
    let challenge = password_step(&t, &from(2)).await;
    let earlier = code_step(&t, &challenge, &t.code_now(alice.account.id, -1), &from(2)).await;
    assert!(matches!(earlier, Err(IdentityError::InvalidCode)), "nor one of a step before it: {earlier:?}");
}

#[tokio::test]
async fn a_recovery_code_is_taken_once() {
    let t = TestIdentity::new();
    let (alice, codes) = alice_with_two_factor(&t).await;
    let typed = codes[3].to_uppercase().replace('-', " ");

    let challenge = password_step(&t, &from(2)).await;
    assert!(code_step(&t, &challenge, &typed, &from(2)).await.is_ok());
    assert_eq!(last_audit(&t), (AuditAction::Login.as_str(), Outcome::Success, Some("recovery_code")));
    assert_eq!(t.use_cases.two_factor.execute(&alice).await.unwrap().recovery_codes_left, 9);

    let challenge = password_step(&t, &from(2)).await;
    let again = code_step(&t, &challenge, &codes[3], &from(2)).await;
    assert!(matches!(again, Err(IdentityError::InvalidCode)), "{again:?}");
    assert!(code_step(&t, &challenge, &codes[4], &from(2)).await.is_ok(), "the others still work");
}

#[tokio::test]
async fn three_wrong_codes_end_a_sign_in() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    let challenge = password_step(&t, &from(2)).await;

    for _ in 0..2 {
        let wrong = code_step(&t, &challenge, "000000", &from(2)).await;
        assert!(matches!(wrong, Err(IdentityError::InvalidCode)), "{wrong:?}");
    }
    // The third says it is over.
    let third = code_step(&t, &challenge, "000000", &from(2)).await;
    assert!(matches!(third, Err(IdentityError::SignInExpired)), "{third:?}");
    assert_eq!(last_audit(&t), (AuditAction::Login.as_str(), Outcome::Failure, Some("wrong_code")));

    let ended = code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await;
    assert!(matches!(ended, Err(IdentityError::SignInExpired)), "{ended:?}");
}

#[tokio::test]
async fn wrong_codes_count_against_the_account_lock_and_a_password_does_not_lift_it() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;

    for _ in 0..5 {
        let challenge = password_step(&t, &from(2)).await;
        let _ = code_step(&t, &challenge, "000000", &from(2)).await;
    }

    let locked = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(3)).await;
    assert!(matches!(locked, Err(IdentityError::TooManyAttempts { .. })), "{locked:?}");
    t.clock.advance(Duration::from_secs(15 * 60));
    let challenge = password_step(&t, &from(2)).await;
    assert!(code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await.is_ok());
}

#[tokio::test]
async fn the_code_comes_from_the_address_the_password_came_from() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    let challenge = password_step(&t, &from(2)).await;
    let code = t.code_now(alice.account.id, 0);

    let elsewhere = code_step(&t, &challenge, &code, &from(3)).await;

    assert!(matches!(elsewhere, Err(IdentityError::SignInExpired)), "{elsewhere:?}");
    assert!(code_step(&t, &challenge, &code, &from(2)).await.is_ok(), "nor does it end the sign-in");
}

#[tokio::test]
async fn a_sign_in_waits_five_minutes_for_its_code() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    let challenge = password_step(&t, &from(2)).await;

    t.clock.advance(Duration::from_secs(5 * 60));

    let expired = code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await;
    assert!(matches!(expired, Err(IdentityError::SignInExpired)), "{expired:?}");
}

#[tokio::test]
async fn an_unknown_token_is_an_expired_sign_in() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    let made_up = SecondFactorChallenge { token: Secret::new("token-99"), expires_at: t.clock.now() };

    let refused = code_step(&t, &made_up, &t.code_now(alice.account.id, 0), &from(2)).await;

    assert!(matches!(refused, Err(IdentityError::SignInExpired)), "{refused:?}");
}

// Changes while a sign-in waits for its code

#[tokio::test]
async fn a_password_changed_meanwhile_ends_the_sign_in() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    let challenge = password_step(&t, &from(2)).await;
    let input = ChangePasswordInput {
        current_password: Secret::new(PASSWORD),
        new_password: Secret::new("a much better passphrase"),
    };
    t.use_cases.change_password.execute(&alice, input, &from(9)).await.unwrap();

    let refused = code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await;

    assert!(matches!(refused, Err(IdentityError::SignInExpired)), "{refused:?}");
}

#[tokio::test]
async fn a_second_factor_reset_and_set_up_again_meanwhile_ends_the_sign_in() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let (alice, _) = alice_with_two_factor(&t).await;
    let challenge = password_step(&t, &from(2)).await;

    t.use_cases.reset_two_factor.execute(&admin, alice.account.id, &from(9)).await.unwrap();
    let alice = password_login(&t.use_cases.login, login_input("alice", PASSWORD), &from(4)).await.unwrap();
    turn_on(&t, &alice.session).await;

    let refused = code_step(&t, &challenge, &t.code_now(alice.session.account.id, 0), &from(2)).await;
    assert!(matches!(refused, Err(IdentityError::SignInExpired)), "{refused:?}");
}

#[tokio::test]
async fn an_account_disabled_meanwhile_starts_no_session() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    let challenge = password_step(&t, &from(2)).await;
    t.accounts.update(alice.account.id, |a| a.disabled = true);

    let refused = code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await;

    assert!(matches!(refused, Err(IdentityError::AccountDisabled)), "{refused:?}");
    assert_eq!(last_audit(&t), (AuditAction::Login.as_str(), Outcome::Denied, Some("disabled")));
}

#[tokio::test]
async fn an_account_deleted_meanwhile_starts_no_session() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let (alice, _) = alice_with_two_factor(&t).await;
    let challenge = password_step(&t, &from(2)).await;
    let code = t.code_now(alice.account.id, 0);

    t.use_cases.delete_account.execute(&admin, alice.account.id, &from(9)).await.unwrap();

    let refused = code_step(&t, &challenge, &code, &from(2)).await;
    assert!(matches!(refused, Err(IdentityError::SignInExpired)), "{refused:?}");
    assert!(t.two_factor.get(alice.account.id).is_none(), "its second factor went with it");
}

// Turning it off, new recovery codes

#[tokio::test]
async fn turning_the_second_factor_off_takes_the_password_and_a_code() {
    let t = TestIdentity::new();
    let (alice, codes) = alice_with_two_factor(&t).await;
    let elsewhere = password_step(&t, &from(2)).await;
    let elsewhere = code_step(&t, &elsewhere, &t.code_now(alice.account.id, 0), &from(2))
        .await
        .unwrap()
        .session
        .session_id;
    let disable = |password: &str, code: &str| {
        let (password, code) = (Secret::new(password), Secret::new(code));
        let alice = alice.clone();
        let t = &t;
        async move { t.use_cases.disable_two_factor.execute(&alice, &password, &code, &from(9)).await }
    };

    let wrong_password = disable("wrong password", &codes[0]).await;
    assert!(matches!(wrong_password, Err(IdentityError::WrongPassword)), "{wrong_password:?}");
    let wrong_code = disable(PASSWORD, "000000").await;
    assert!(matches!(wrong_code, Err(IdentityError::InvalidCode)), "{wrong_code:?}");
    assert_eq!(
        last_audit(&t),
        (AuditAction::TwoFactorDisable.as_str(), Outcome::Failure, Some("wrong_code"))
    );

    disable(PASSWORD, &codes[0]).await.unwrap();

    assert!(!t.use_cases.two_factor.execute(&alice).await.unwrap().on);
    assert!(t.two_factor.get(alice.account.id).is_none());
    assert!(t.sessions.all().iter().all(|s| s.id != elsewhere), "the other sessions ended");
    assert!(password_login(&t.use_cases.login, login_input("alice", PASSWORD), &from(2)).await.is_ok());
    let off = disable(PASSWORD, &codes[1]).await;
    assert!(matches!(off, Err(IdentityError::TwoFactorOff)), "{off:?}");
}

#[tokio::test]
async fn new_recovery_codes_replace_the_old_ones() {
    let t = TestIdentity::new();
    let (alice, old) = alice_with_two_factor(&t).await;
    let waiting = password_step(&t, &from(2)).await;
    let by_recovery = code_step(&t, &waiting, &old[9], &from(2)).await.unwrap().session.session_id;
    let waiting = password_step(&t, &from(2)).await;

    let by_recovery_code = t
        .use_cases
        .renew_recovery_codes
        .execute(&alice, &Secret::new(PASSWORD), &Secret::new(&old[0]), &from(9))
        .await;
    assert!(matches!(by_recovery_code, Err(IdentityError::InvalidCode)), "only the app's code");
    let code = Secret::new(t.code_now(alice.account.id, 0));
    let wrong = Secret::new("wrong password");
    let without_password = t.use_cases.renew_recovery_codes.execute(&alice, &wrong, &code, &from(9)).await;
    assert!(matches!(without_password, Err(IdentityError::WrongPassword)), "{without_password:?}");
    let entry = (AuditAction::RecoveryCodesRenew.as_str(), Outcome::Failure, Some("wrong_password"));
    assert_eq!(last_audit(&t), entry);
    let new = t
        .use_cases
        .renew_recovery_codes
        .execute(&alice, &Secret::new(PASSWORD), &code, &from(9))
        .await
        .unwrap();

    assert_eq!(new.len(), 10);
    assert!(new.iter().all(|code| !old.contains(code)));
    assert_eq!(last_audit(&t), (AuditAction::RecoveryCodesRenew.as_str(), Outcome::Success, None));
    assert!(t.sessions.all().iter().all(|s| s.id != by_recovery), "the other sessions ended");
    let ended = code_step(&t, &waiting, &new[1], &from(2)).await;
    assert!(matches!(ended, Err(IdentityError::SignInExpired)), "and the waiting sign-ins: {ended:?}");
    let challenge = password_step(&t, &from(2)).await;
    let gone = code_step(&t, &challenge, &old[1], &from(2)).await;
    assert!(matches!(gone, Err(IdentityError::InvalidCode)), "{gone:?}");
    let replayed = code_step(&t, &challenge, code.expose(), &from(2)).await;
    assert!(matches!(replayed, Err(IdentityError::InvalidCode)), "its code is taken: {replayed:?}");
    assert!(code_step(&t, &challenge, &new[0], &from(2)).await.is_ok());
}

// Administrators

#[tokio::test]
async fn an_administrator_resets_another_accounts_second_factor() {
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let (alice, _) = alice_with_two_factor(&t).await;
    let bob = signed_in(&t, "bob", Role::User).await;
    let listed = t.use_cases.two_factor_accounts.execute(&admin).await.unwrap();
    assert_eq!(listed, vec![alice.account.id]);

    let forbidden = t.use_cases.reset_two_factor.execute(&bob, alice.account.id, &from(9)).await;
    assert!(matches!(forbidden, Err(IdentityError::Forbidden)), "{forbidden:?}");
    let not_listed = t.use_cases.two_factor_accounts.execute(&bob).await;
    assert!(matches!(not_listed, Err(IdentityError::Forbidden)), "{not_listed:?}");
    let own = t.use_cases.reset_two_factor.execute(&admin, admin.account.id, &from(9)).await;
    assert!(matches!(own, Err(IdentityError::OwnAccount)), "{own:?}");
    let off = t.use_cases.reset_two_factor.execute(&admin, bob.account.id, &from(9)).await;
    assert!(matches!(off, Err(IdentityError::TwoFactorOff)), "{off:?}");

    t.use_cases.reset_two_factor.execute(&admin, alice.account.id, &from(9)).await.unwrap();

    assert!(t.sessions.all().iter().all(|s| s.account_id != alice.account.id), "her sessions ended");
    let entry = t.audit.entries().pop().unwrap();
    assert_eq!(
        (entry.action, entry.target.as_deref()),
        (AuditAction::TwoFactorReset.as_str(), Some("alice"))
    );
    assert!(password_login(&t.use_cases.login, login_input("alice", PASSWORD), &from(2)).await.is_ok());
    assert!(t.use_cases.two_factor_accounts.execute(&admin).await.unwrap().is_empty());
}

/// With the account lock off, wrong second codes from many networks are
/// still bounded per account, and the owner signing in meanwhile does not
/// give them new tries.
#[tokio::test]
async fn wrong_codes_are_bounded_per_account_from_anywhere_whatever_the_lock() {
    use crate::domain::SecuritySettings;
    let t = TestIdentity::new();
    let admin = signed_in(&t, "admin", Role::Admin).await;
    let (alice, _) = alice_with_two_factor(&t).await;
    let off = SecuritySettings { account_lock: None, ..SecuritySettings::default() };
    t.use_cases.set_security.execute(&admin, off, &from(9)).await.unwrap();
    // One address of a /64 each.
    let net = |n: u16| RequestContext {
        client_ip: Some(std::net::IpAddr::from([0x2001, 0xdb8, n, 0, 0, 0, 0, 1])),
        ..from(9)
    };

    let mut wrong = 0;
    for n in 1..=4u16 {
        let challenge = password_step(&t, &net(n)).await;
        for _ in 0..3 {
            if code_step(&t, &challenge, "000000", &net(n)).await.is_ok() {
                panic!("a wrong code");
            }
            wrong += 1;
            if wrong == 10 {
                break;
            }
        }
        if wrong == 10 {
            break;
        }
    }
    // The owner signs in in between.
    let challenge = password_step(&t, &from(2)).await;
    let right = Secret::new(t.code_now(alice.account.id, 0));
    let refused = t.use_cases.complete_sign_in.execute(&challenge.token, &right, &from(2)).await;
    assert!(matches!(refused, Err(IdentityError::TooManyAttempts { .. })), "{refused:?}");
    let challenge = password_step(&t, &net(99)).await;
    let guess = code_step(&t, &challenge, "000000", &net(99)).await;
    assert!(matches!(guess, Err(IdentityError::TooManyAttempts { .. })), "{guess:?}");

    // An hour on, codes are taken again.
    t.clock.advance(Duration::from_secs(60 * 60));
    let challenge = password_step(&t, &from(2)).await;
    assert!(code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await.is_ok());
}

/// Someone who knows Alice's password but has no code sends wrong codes:
/// three a sign-in from a dozen addresses, pacing themselves under the
/// address limits, the account lock turned off. Thirty, the most a day
/// takes from the app.
async fn others_use_up_the_wrong_codes(t: &TestIdentity) {
    let mut wrong = 0;
    for hour in 0..3u8 {
        for sign_in in 0..4u8 {
            let attacker = from(60 + hour * 4 + sign_in);
            let challenge = password_step(t, &attacker).await;
            for _ in 0..3 {
                let refused = code_step(t, &challenge, "000000", &attacker).await;
                if matches!(refused, Err(IdentityError::TooManyAttempts { .. })) {
                    break;
                }
                assert!(refused.is_err());
                wrong += 1;
            }
        }
        t.clock.advance(Duration::from_secs(60 * 60));
    }
    assert_eq!(wrong, 30);
}

async fn without_the_account_lock(t: &TestIdentity) {
    use crate::domain::SecuritySettings;
    let admin = signed_in(t, "admin", Role::Admin).await;
    let off = SecuritySettings { account_lock: None, ..SecuritySettings::default() };
    t.use_cases.set_security.execute(&admin, off, &from(9)).await.unwrap();
}

/// Wrong codes others send keep the owner from codes of the app for the
/// day, but not from the recovery codes: those are for when the app is out
/// of reach, and guessing one is hopeless.
#[tokio::test]
async fn whoever_knows_the_password_cannot_keep_the_owner_from_a_recovery_code() {
    let t = TestIdentity::new();
    let (alice, codes) = alice_with_two_factor(&t).await;
    without_the_account_lock(&t).await;
    others_use_up_the_wrong_codes(&t).await;

    let challenge = password_step(&t, &from(2)).await;
    let app = code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await;
    assert!(matches!(app, Err(IdentityError::TooManyAttempts { .. })), "{app:?}");
    let recovery = code_step(&t, &challenge, &codes[0], &from(2)).await;
    assert!(recovery.is_ok(), "the owner's recovery code: {recovery:?}");
    assert_eq!(last_audit(&t), (AuditAction::Login.as_str(), Outcome::Success, Some("recovery_code")));
}

/// Recovery codes are left out of the account's limit of codes from the
/// app, not out of the throttles: a wrong one counts as a wrong password
/// does, against the account lock and the address.
#[tokio::test]
async fn wrong_recovery_codes_count_as_wrong_passwords_do() {
    let t = TestIdentity::new();
    alice_with_two_factor(&t).await;
    // Of the form of a recovery code, and none of them.
    let guess = "zzzzz-zzzzz";
    // The account lock: five failures within 15 minutes, from anywhere.
    let mut refused = None;
    for n in 0..4u8 {
        let request = from(30 + n);
        let challenge = password_step(&t, &request).await;
        for _ in 0..2 {
            match code_step(&t, &challenge, guess, &request).await {
                Err(IdentityError::InvalidCode) => {}
                other => {
                    refused = Some(other);
                    break;
                }
            }
        }
        if refused.is_some() {
            break;
        }
    }
    assert!(matches!(refused, Some(Err(IdentityError::TooManyAttempts { .. }))), "{refused:?}");
    let locked = t.use_cases.login.execute(login_input("alice", PASSWORD), &from(40)).await;
    assert!(matches!(locked, Err(IdentityError::TooManyAttempts { .. })), "{locked:?}");

    // Without the lock, one address takes no more than its limit.
    let t = TestIdentity::new();
    let (_, codes) = alice_with_two_factor(&t).await;
    without_the_account_lock(&t).await;
    let mut wrong = 0;
    let stopped = loop {
        let challenge = match t.use_cases.login.execute(login_input("alice", PASSWORD), &from(50)).await {
            Ok(LoginOutcome::SecondFactor(challenge)) => challenge,
            other => break other.map(|_| ()),
        };
        let mut ended = None;
        for _ in 0..3 {
            match code_step(&t, &challenge, guess, &from(50)).await {
                Err(IdentityError::InvalidCode | IdentityError::SignInExpired) => wrong += 1,
                other => {
                    ended = Some(other.map(|_| ()));
                    break;
                }
            }
        }
        if let Some(ended) = ended {
            break ended;
        }
        assert!(wrong <= 20, "{wrong} wrong recovery codes from one address");
    };
    assert!(
        matches!(stopped, Err(IdentityError::TooManyAttempts { .. } | IdentityError::AddressBlocked)),
        "{stopped:?}"
    );
    assert!(wrong < 20, "{wrong}");
    // Elsewhere, the owner still gets in with one.
    let challenge = password_step(&t, &from(2)).await;
    assert!(code_step(&t, &challenge, &codes[1], &from(2)).await.is_ok());
}

/// Kept from the app's codes by others' wrong ones, the owner asks an
/// administrator, who clears the account's failed sign-ins: the app's code
/// is taken at once. Audited; not for others than administrators.
#[tokio::test]
async fn an_administrator_clears_an_accounts_failed_sign_ins() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    without_the_account_lock(&t).await;
    others_use_up_the_wrong_codes(&t).await;
    let challenge = password_step(&t, &from(2)).await;
    let app = code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await;
    assert!(matches!(app, Err(IdentityError::TooManyAttempts { .. })), "{app:?}");

    let admin = signed_in_again(&t, "admin").await;
    let bob = signed_in(&t, "bob", Role::User).await;
    let forbidden = t.use_cases.clear_failed_sign_ins.execute(&bob, alice.account.id, &from(9)).await;
    assert!(matches!(forbidden, Err(IdentityError::Forbidden)), "{forbidden:?}");
    let nobody = AccountId::new(999);
    let missing = t.use_cases.clear_failed_sign_ins.execute(&admin, nobody, &from(9)).await;
    assert!(matches!(missing, Err(IdentityError::AccountNotFound)), "{missing:?}");
    t.use_cases.clear_failed_sign_ins.execute(&admin, alice.account.id, &from(9)).await.unwrap();
    let entry = t.audit.entries().pop().unwrap();
    assert_eq!(
        (entry.action, entry.target.as_deref()),
        (AuditAction::FailedSignInsClear.as_str(), Some("alice"))
    );

    assert!(code_step(&t, &challenge, &t.code_now(alice.account.id, 0), &from(2)).await.is_ok());
}

/// Sign-ins of another account, or from elsewhere, never end a sign-in
/// waiting for its code: each account and each network makes room among
/// its own.
#[tokio::test]
async fn others_sign_ins_do_not_end_a_waiting_one() {
    let t = TestIdentity::new();
    let (alice, _) = alice_with_two_factor(&t).await;
    let mallory = signed_in(&t, "mallory", Role::ReadOnly).await;
    turn_on(&t, &mallory).await;

    let waiting = password_step(&t, &from(2)).await;
    t.clock.advance(Duration::from_secs(1));
    for _ in 0..50 {
        let _ = t.use_cases.login.execute(login_input("mallory", PASSWORD), &from(66)).await.unwrap();
    }
    assert!(code_step(&t, &waiting, &t.code_now(alice.account.id, 0), &from(2)).await.is_ok());
}

/// The same right code sent twice at once starts one session: the second
/// finds the sign-in over once it gets the lock.
#[tokio::test]
async fn a_sign_in_starts_one_session_however_often_it_is_sent() {
    let t = TestIdentity::new();
    let (alice, codes) = alice_with_two_factor(&t).await;
    let challenge = password_step(&t, &from(2)).await;
    let (request, now) = (from(2), t.code_now(alice.account.id, 0));
    let first = code_step(&t, &challenge, &codes[0], &request);
    let second = code_step(&t, &challenge, &now, &request);
    let (first, second) = tokio::join!(first, second);
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1, "{first:?} {second:?}");
}
