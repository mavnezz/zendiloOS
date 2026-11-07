import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import type { SessionInfo } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import { logout } from '../api'
import { useSessionStore } from '../store'
import PasswordChangePage from './PasswordChangePage.vue'

vi.mock('../api', () => ({ logout: vi.fn(), fetchSession: vi.fn() }))

const session: SessionInfo = {
  account: {
    id: 1,
    name: 'alice',
    full_name: null,
    role: 'user',
    permissions: [],
    password_change_required: true,
    password_locked: false,
  },
  csrf_token: 'csrf-1',
  expires_at: 0,
}

beforeEach(() => {
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  vi.mocked(logout).mockReset()
})

async function render() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/password', name: 'password', component: PasswordChangePage },
      { path: '/login', name: 'login', component: { template: '<div />' } },
    ],
  })
  await router.push('/password')
  useSessionStore().start(session)
  const queryClient = new QueryClient()
  const wrapper = mount(PasswordChangePage, {
    global: { plugins: [router, i18n, [VueQueryPlugin, { queryClient }]] },
  })
  return { wrapper, router, queryClient }
}

describe('PasswordChangePage', () => {
  it('does not claim an administrator set a password that only grew old', async () => {
    const { wrapper } = await render()
    const intro = wrapper.find('p').text()
    expect(intro).toContain('alice')
    expect(intro).toContain('maximum age')
  })

  it('signs out and forgets what this browser held of the account', async () => {
    vi.mocked(logout).mockResolvedValue()
    const { wrapper, router, queryClient } = await render()
    queryClient.setQueryData(['shares'], [{ name: 'photos' }])

    await wrapper.find('button[type="button"], button:not([type])').trigger('click')
    await flushPromises()

    expect(logout).toHaveBeenCalledOnce()
    expect(queryClient.getQueryData(['shares'])).toBeUndefined()
    expect(router.currentRoute.value.name).toBe('login')
  })
})
