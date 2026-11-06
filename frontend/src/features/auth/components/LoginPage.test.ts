import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import { ApiProblem } from '@/shared/api/client'
import type { SessionInfo } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import { login } from '../api'
import { useSessionStore } from '../store'
import LoginPage from './LoginPage.vue'

vi.mock('../api', () => ({ login: vi.fn() }))

const session: SessionInfo = {
  account: {
    id: 1,
    name: 'alice',
    full_name: null,
    role: 'admin',
    permissions: ['system.view'],
    password_change_required: false,
    password_locked: false,
  },
  csrf_token: 'csrf-1',
  expires_at: 0,
}

async function renderLoginPage() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/login', name: 'login', component: LoginPage },
      { path: '/', name: 'desktop', component: { template: '<div />' } },
    ],
  })
  await router.push('/login')
  const wrapper = mount(LoginPage, { global: { plugins: [router, i18n] } })
  await wrapper.find('#login-name').setValue('alice')
  await wrapper.find('#login-password').setValue('correct horse battery')
  await wrapper.find('form').trigger('submit')
  await flushPromises()
  return { wrapper, router }
}

beforeEach(() => {
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  vi.mocked(login).mockReset()
})

describe('LoginPage', () => {
  it('signs in and opens the desktop', async () => {
    vi.mocked(login).mockResolvedValue(session)

    const { router } = await renderLoginPage()

    expect(login).toHaveBeenCalledWith('alice', 'correct horse battery')
    expect(useSessionStore().info?.account.name).toBe('alice')
    expect(router.currentRoute.value.name).toBe('desktop')
  })

  it('explains a failed login and clears the password', async () => {
    vi.mocked(login).mockRejectedValue(new ApiProblem(401, 'invalid_credentials'))

    const { wrapper, router } = await renderLoginPage()

    expect(wrapper.find('[role="alert"]').text()).toBe('Wrong name or password.')
    expect((wrapper.find('#login-password').element as HTMLInputElement).value).toBe('')
    expect(router.currentRoute.value.name).toBe('login')
  })
})
