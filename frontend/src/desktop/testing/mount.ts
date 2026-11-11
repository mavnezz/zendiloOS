import type { Component } from 'vue'

import { useSessionStore } from '@/features/auth/store'
import { mountApp as mountWith, type Options } from '@/shared/test-support/mount'

export * from '@/shared/test-support/mount'

/** Every permission an administrator has. */
export const ADMIN = [
  'system.view',
  'system.manage',
  'storage.view',
  'storage.manage',
  'shares.manage',
  'users.manage',
  'containers.manage',
  'vms.manage',
  'apps.manage',
  'files.read',
  'files.write',
  'logs.view',
  'account.self',
]

/** Signs `permissions` in, for components that ask the session. */
export function signIn(permissions: string[] = ADMIN): void {
  useSessionStore().start({
    account: {
      id: 1,
      name: 'alice',
      full_name: 'Alice Admin',
      role: 'admin',
      permissions,
      password_change_required: false,
      password_locked: false,
      created_at: 1_700_000_000,
    },
    csrf_token: 'csrf',
    expires_at: 2_000_000_000,
  })
}

/** As the shared `mountApp`, signed in as an administrator unless told. */
export function mountApp<C extends Component>(
  component: C,
  options: Options<C> & { permissions?: string[] } = {},
) {
  const { permissions, ...rest } = options
  return mountWith(component, { ...rest, before: () => signIn(permissions) })
}
