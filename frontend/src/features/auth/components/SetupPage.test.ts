import {
  callsTo,
  cleanup,
  mountApp,
  on,
  page,
  problem,
  settle,
  type,
} from '@/shared/testing/server'

import { beforeEach, describe, expect, it } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import type { SessionInfo } from '@/shared/api/types'

import { useSessionStore } from '../store'
import ChangePasswordForm from './ChangePasswordForm.vue'
import SetupPage from './SetupPage.vue'

const session: SessionInfo = {
  account: {
    id: 1,
    name: 'admin',
    full_name: null,
    role: 'admin',
    permissions: ['system.view'],
    password_change_required: false,
    password_locked: false,
  },
  csrf_token: 'csrf-1',
  expires_at: 0,
}

function memoryRouter() {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/setup', name: 'setup', component: SetupPage },
      { path: '/login', name: 'login', component: { template: '<p>sign in</p>' } },
      { path: '/password', name: 'password', component: { template: '<p>password</p>' } },
      { path: '/', name: 'desktop', component: { template: '<p>desktop</p>' } },
    ],
  })
}

async function setup() {
  const router = memoryRouter()
  await router.push('/setup')
  mountApp(SetupPage, { global: { plugins: [router] } })
  await settle()
  await type('#setup-code', 'ABCD-EFGH-IJKL')
  await type('#setup-name', 'admin')
  await type('#setup-password', 'correct horse battery')
  return router
}

function submit(selector: string): Promise<void> {
  document.body.querySelector<HTMLElement>(selector)!.closest('form')!.requestSubmit()
  return settle()
}

beforeEach(() => cleanup())

describe('the setup', () => {
  it('creates the first administrator and goes to the desktop', async () => {
    on('POST', '/api/v1/setup', () => session)
    const router = await setup()
    await type('#setup-password-repeat', 'correct horse battery')
    await submit('#setup-name')

    expect(callsTo('POST', '/api/v1/setup')[0]!.body).toEqual({
      setup_code: 'ABCD-EFGH-IJKL',
      name: 'admin',
      password: 'correct horse battery',
    })
    expect(useSessionStore().info?.csrf_token).toBe('csrf-1')
    expect(router.currentRoute.value.name).toBe('desktop')
  })

  it('does not send passwords that differ', async () => {
    await setup()
    await type('#setup-password-repeat', 'something else')
    expect(page()).toContain('The passwords do not match.')
    await submit('#setup-name')
    expect(callsTo('POST', '/api/v1/setup')).toHaveLength(0)
  })

  it('leads to the sign-in when someone completed it meanwhile, and shows other refusals', async () => {
    on('POST', '/api/v1/setup', () => problem(409, 'setup_completed'))
    await setup()
    await type('#setup-password-repeat', 'correct horse battery')
    await submit('#setup-name')
    expect(page()).toContain('The setup has already been completed.')
    expect(page()).toContain('Go to sign-in')

    on('POST', '/api/v1/setup', () => problem(403, 'invalid_setup_code'))
    await submit('#setup-name')
    expect(page()).toContain('The setup code is wrong.')
    expect(page()).not.toContain('Go to sign-in')
  })
})

describe('the password form', () => {
  it('changes the password, empties itself and says so', async () => {
    on('POST', '/api/v1/auth/password', () => undefined)
    const wrapper = mountApp(ChangePasswordForm)
    await settle()
    await type('#password-current', 'old password here')
    await type('#password-new', 'a new long password')
    await type('#password-repeat', 'a new long password')
    await submit('#password-new')

    expect(callsTo('POST', '/api/v1/auth/password')[0]!.body).toEqual({
      current_password: 'old password here',
      new_password: 'a new long password',
    })
    expect(document.body.querySelector<HTMLInputElement>('#password-new')!.value).toBe('')
    expect(wrapper.emitted('changed')).toHaveLength(1)
  })

  it('refuses differing passwords and shows a wrong current one', async () => {
    on('POST', '/api/v1/auth/password', () => problem(403, 'wrong_password'))
    mountApp(ChangePasswordForm)
    await settle()
    await type('#password-current', 'old')
    await type('#password-new', 'a new long password')
    await type('#password-repeat', 'another')
    expect(page()).toContain('The passwords do not match.')
    await submit('#password-new')
    expect(callsTo('POST', '/api/v1/auth/password')).toHaveLength(0)
    await type('#password-repeat', 'a new long password')
    await submit('#password-new')
    expect(page()).toContain('The current password is wrong.')
  })
})
