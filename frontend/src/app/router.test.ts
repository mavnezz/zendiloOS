import { answer, cleanup, mountApp, on, problem } from '@/shared/testing/server'

import { beforeEach, describe, expect, it } from 'vitest'
import { nextTick } from 'vue'

import { useSessionStore } from '@/features/auth/store'
import type { SessionInfo } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { staleBuild } from '@/shared/lib/stale-build'

import App from './App.vue'
import { installNavigationGuard } from './navigation'
import { createAppRouter } from './router'

const session: SessionInfo = {
  account: {
    id: 1,
    name: 'admin',
    full_name: null,
    role: 'admin',
    permissions: ['system.view'],
    password_change_required: false,
    password_locked: false,
    created_at: 1_700_000_000,
  },
  csrf_token: 'csrf-1',
  expires_at: 0,
}

beforeEach(() => cleanup())

describe('the way to each page', () => {
  async function start(path: string) {
    const router = createAppRouter()
    installNavigationGuard(router, useSessionStore())
    await router.push(path)
    await router.isReady()
    return router
  }

  it('sends a browser without a session to the setup while it is needed, else to the sign-in', async () => {
    on('GET', '/api/v1/auth/session', () => problem(401, 'unauthenticated'))
    answer('GET', '/api/v1/setup', { required: true })
    mountApp(App, { global: { plugins: [] } })
    let router = await start('/')
    expect(router.currentRoute.value.name).toBe('setup')

    cleanup()
    on('GET', '/api/v1/auth/session', () => problem(401, 'unauthenticated'))
    answer('GET', '/api/v1/setup', { required: false })
    mountApp(App)
    router = await start('/password')
    expect(router.currentRoute.value.name).toBe('login')
  })

  it('sends a signed-in account to choose its password, or to the desktop', async () => {
    answer('GET', '/api/v1/auth/session', {
      ...session,
      account: { ...session.account, password_change_required: true },
    })
    mountApp(App)
    const router = await start('/login')
    expect(router.currentRoute.value.name).toBe('password')
    expect(useSessionStore().can('system.view')).toBe(true)
    expect(useSessionStore().can('users.manage')).toBe(false)
  })

  it('shows the sign-in when the server cannot be reached', async () => {
    on('GET', '/api/v1/auth/session', () => problem(502, 'bad_gateway'))
    mountApp(App)
    const router = await start('/')
    expect(router.currentRoute.value.name).toBe('login')
  })

  it('asks to reload when parts of the interface failed to load again', async () => {
    const wrapper = mountApp(App, { global: { stubs: { RouterView: true } } })
    expect(wrapper.text()).not.toContain(i18n.global.t('app.staleBuild'))
    staleBuild.value = true
    await nextTick()
    expect(wrapper.text()).toContain(i18n.global.t('app.staleBuild'))
    expect(wrapper.find('button').text()).toBe(i18n.global.t('app.reload'))
    staleBuild.value = false
  })

  it('signs out even when the server does not answer', async () => {
    on('POST', '/api/v1/auth/logout', () => problem(502, 'bad_gateway'))
    mountApp(App)
    const store = useSessionStore()
    store.start(session)
    await expect(store.signOut()).rejects.toThrow()
    expect(store.signedIn).toBe(false)
  })
})
