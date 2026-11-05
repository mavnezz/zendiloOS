import type { Router } from 'vue-router'

import { fetchSetupRequired } from '@/features/auth/api'
import type { useSessionStore } from '@/features/auth/store'

export type Page = 'setup' | 'login' | 'password' | 'desktop'

export interface AuthState {
  signedIn: boolean
  setupRequired: boolean
  /** An administrator set the password; the account must choose its own. */
  passwordChangeRequired: boolean
}

/**
 * Where a navigation to `target` has to go instead, or `null` to let it
 * through. Exactly one page fits each state.
 */
export function redirectFor(target: Page, state: AuthState): Page | null {
  const page: Page = state.signedIn
    ? state.passwordChangeRequired
      ? 'password'
      : 'desktop'
    : state.setupRequired
      ? 'setup'
      : 'login'
  return target === page ? null : page
}

async function authState(session: ReturnType<typeof useSessionStore>): Promise<AuthState> {
  try {
    await session.ensureLoaded()
    if (session.signedIn) {
      const passwordChangeRequired = session.info?.account.password_change_required ?? false
      return { signedIn: true, setupRequired: false, passwordChangeRequired }
    }
    return {
      signedIn: false,
      setupRequired: await fetchSetupRequired(),
      passwordChangeRequired: false,
    }
  } catch {
    // Server unreachable: the sign-in page will say so when used.
    return { signedIn: false, setupRequired: false, passwordChangeRequired: false }
  }
}

export function installNavigationGuard(
  router: Router,
  session: ReturnType<typeof useSessionStore>,
): void {
  router.beforeEach(async (to) => {
    const redirect = redirectFor(to.name as Page, await authState(session))
    return redirect === null ? true : { name: redirect }
  })
}
