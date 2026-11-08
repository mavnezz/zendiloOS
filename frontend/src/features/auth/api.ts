import { ApiProblem, api, unwrap } from '@/shared/api/client'
import type { SessionInfo, SetupRequest, SetupStatus } from '@/shared/api/types'

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

export function login(name: string, password: string): Promise<SessionInfo> {
  return unwrap(api.POST('/api/v1/auth/login', { body: { name, password } }))
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
