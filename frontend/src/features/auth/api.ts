import { ApiProblem, api, unwrap } from '@/shared/api/client'
import { passphraseHeader } from '@/shared/lib/passphrase'
import type {
  RestoreResult,
  SecondFactorChallenge,
  SessionInfo,
  SetupRequest,
  SetupStatus,
  TwoFactorSetup,
  TwoFactorStatus,
} from '@/shared/api/types'

/** The current session, or `null` if nobody is signed in. */
export async function fetchSession(): Promise<SessionInfo | null> {
  try {
    return await unwrap(api.GET('/api/v1/auth/session'))
  } catch (error) {
    if (error instanceof ApiProblem && error.status === 401) return null
    throw error
  }
}

export async function fetchSetupRequired(): Promise<boolean> {
  return (await fetchSetupStatus()).required
}

/** Whether the setup is pending, and whether it asks for the console's code. */
export function fetchSetupStatus(): Promise<SetupStatus> {
  return unwrap(api.GET('/api/v1/setup'))
}

export function completeSetup(request: SetupRequest): Promise<SessionInfo> {
  return unwrap(api.POST('/api/v1/setup', { body: request }))
}

/** Restores a configuration backup instead of creating the first administrator. */
export function restoreSetup(
  backup: Blob,
  passphrase: string,
  setupCode: string | null,
): Promise<RestoreResult> {
  return unwrap(
    api.POST('/api/v1/setup/restore', {
      params: {
        header: {
          'x-zendiloos-passphrase': passphraseHeader(passphrase),
          ...(setupCode ? { 'x-zendiloos-setup-code': setupCode } : {}),
        },
      },
      body: backup as never,
      bodySerializer: (body: unknown) => body as BodyInit,
      headers: { 'Content-Type': 'application/octet-stream' },
    }),
  )
}

/** What came of the password: a session, or the second factor asked for. */
export type LoginResult =
  | { kind: 'signed_in'; session: SessionInfo }
  | { kind: 'second_factor'; challenge: SecondFactorChallenge }

export async function login(name: string, password: string): Promise<LoginResult> {
  const answer = await unwrap(api.POST('/api/v1/auth/login', { body: { name, password } }))
  return 'token' in answer
    ? { kind: 'second_factor', challenge: answer }
    : { kind: 'signed_in', session: answer }
}

/** The second step of a sign-in: a code from the app, or a recovery code. */
export function completeSignIn(token: string, code: string): Promise<SessionInfo> {
  return unwrap(api.POST('/api/v1/auth/login/second-factor', { body: { token, code } }))
}

export function fetchTwoFactor(): Promise<TwoFactorStatus> {
  return unwrap(api.GET('/api/v1/auth/two-factor'))
}

/** A new secret for the authenticator app, waiting for its first code. */
export function beginTwoFactor(password: string): Promise<TwoFactorSetup> {
  return unwrap(api.POST('/api/v1/auth/two-factor/setup', { body: { password } }))
}

/** Turns two-factor sign-in on; gives the recovery codes, shown this once. */
export async function confirmTwoFactor(code: string): Promise<string[]> {
  return (await unwrap(api.POST('/api/v1/auth/two-factor/confirm', { body: { code } })))
    .recovery_codes
}

export async function disableTwoFactor(password: string, code: string): Promise<void> {
  await unwrap(api.POST('/api/v1/auth/two-factor/disable', { body: { password, code } }))
}

/** New recovery codes; the old ones stop working. */
export async function renewRecoveryCodes(password: string, code: string): Promise<string[]> {
  const body = { password, code }
  return (await unwrap(api.POST('/api/v1/auth/two-factor/recovery-codes', { body }))).recovery_codes
}

export async function logout(): Promise<void> {
  await unwrap(api.POST('/api/v1/auth/logout'))
}

export async function changePassword(currentPassword: string, newPassword: string): Promise<void> {
  await unwrap(
    api.POST('/api/v1/auth/password', {
      body: { current_password: currentPassword, new_password: newPassword },
    }),
  )
}
