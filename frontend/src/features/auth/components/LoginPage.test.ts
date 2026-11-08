import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import { ApiProblem } from '@/shared/api/client'
import type { SessionInfo } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import { completeSignIn, login } from '../api'
import { useSessionStore } from '../store'
import LoginPage from './LoginPage.vue'

vi.mock('../api', () => ({ login: vi.fn(), completeSignIn: vi.fn() }))

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

const challenge = { token: 'token-1', expires_at: 1_700_000_300 }

async function renderLoginPage() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/login', name: 'login', component: LoginPage },
      { path: '/', name: 'desktop', component: { template: '<div />' } },
    ],
  })
  await router.push('/login')
  const wrapper = mount(LoginPage, { global: { plugins: [router, i18n] }, attachTo: document.body })
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
  vi.mocked(completeSignIn).mockReset()
  document.body.innerHTML = ''
})

describe('LoginPage', () => {
  it('signs in and opens the desktop', async () => {
    vi.mocked(login).mockResolvedValue({ kind: 'signed_in', session })

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

  it('asks for the code from the app when the account has a second factor', async () => {
    vi.mocked(login).mockResolvedValue({ kind: 'second_factor', challenge })
    vi.mocked(completeSignIn).mockResolvedValue(session)

    const { wrapper, router } = await renderLoginPage()

    expect(router.currentRoute.value.name).toBe('login')
    expect(useSessionStore().info).toBeNull()
    const code = wrapper.find('#login-code')
    expect(code.attributes('autocomplete')).toBe('one-time-code')
    expect(code.attributes('inputmode')).toBe('numeric')
    expect(document.activeElement).toBe(code.element)
    await code.setValue('123456')
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(completeSignIn).toHaveBeenCalledWith('token-1', '123456')
    expect(useSessionStore().info?.account.name).toBe('alice')
    expect(router.currentRoute.value.name).toBe('desktop')
  })

  it('takes a recovery code instead', async () => {
    vi.mocked(login).mockResolvedValue({ kind: 'second_factor', challenge })
    vi.mocked(completeSignIn).mockResolvedValue(session)
    const { wrapper } = await renderLoginPage()

    const toggle = wrapper
      .findAll('button[type="button"]')
      .find((b) => b.text() === 'Use a recovery code')
    await toggle!.trigger('click')

    expect(wrapper.find('label').text()).toBe('Recovery code')
    expect(wrapper.find('#login-code').attributes('autocomplete')).toBe('off')
    await wrapper.find('#login-code').setValue('abcde-fghjk')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(completeSignIn).toHaveBeenCalledWith('token-1', 'abcde-fghjk')
  })

  it('explains a wrong code and lets it be typed again', async () => {
    vi.mocked(login).mockResolvedValue({ kind: 'second_factor', challenge })
    vi.mocked(completeSignIn).mockRejectedValue(new ApiProblem(422, 'invalid_code'))
    const { wrapper } = await renderLoginPage()

    await wrapper.find('#login-code').setValue('000000')
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[role="alert"]').text()).toBe('The code is wrong.')
    expect((wrapper.find('#login-code').element as HTMLInputElement).value).toBe('')
  })

  it('goes back to the password once the sign-in has expired', async () => {
    vi.mocked(login).mockResolvedValue({ kind: 'second_factor', challenge })
    vi.mocked(completeSignIn).mockRejectedValue(new ApiProblem(401, 'sign_in_expired'))
    const { wrapper } = await renderLoginPage()

    await wrapper.find('#login-code').setValue('123456')
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(wrapper.find('#login-password').exists()).toBe(true)
    expect(wrapper.find('[role="alert"]').text()).toBe(
      'The sign-in took too long or was ended. Sign in again.',
    )
  })

  it('goes back to the start on request', async () => {
    vi.mocked(login).mockResolvedValue({ kind: 'second_factor', challenge })
    const { wrapper } = await renderLoginPage()

    const back = wrapper
      .findAll('button[type="button"]')
      .find((b) => b.text() === 'Back to sign-in')
    await back!.trigger('click')

    expect(wrapper.find('#login-password').exists()).toBe(true)
    expect(wrapper.find('#login-code').exists()).toBe(false)
  })
})
