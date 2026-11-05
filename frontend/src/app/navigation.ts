import type { Router } from 'vue-router'

import { fetchSetupRequired } from '@/features/auth/api'
import type { useSessionStore } from '@/features/auth/store'

export type Page = 'setup' | 'login' | 'desktop'

export interface AuthState {
  signedIn: boolean
  setupRequired: boolean
}

/**
 * Where a navigation to `target` has to go instead, or `null` to let it
 * through. Exactly one page fits each state.
 */
export function redirectFor(target: Page, state: AuthState): Page | null {
  const page: Page = state.signedIn ? 'desktop' : state.setupRequired ? 'setup' : 'login'
  return target === page ? null : page
}

async function authState(session: ReturnType<typeof useSessionStore>): Promise<AuthState> {
  try {
    await session.ensureLoaded()
    if (session.signedIn) return { signedIn: true, setupRequired: false }
    return { signedIn: false, setupRequired: await fetchSetupRequired() }
  } catch {
    // Server unreachable: the sign-in page will say so when used.
    return { signedIn: false, setupRequired: false }
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
