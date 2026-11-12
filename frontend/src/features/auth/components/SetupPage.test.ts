import {
  callsTo,
  cleanup,
  click,
  mountApp,
  on,
  page,
  problem,
  settle,
  type,
} from '@/shared/testing/server'

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import type { SessionInfo } from '@/shared/api/types'

import { useSessionStore } from '../store'
import ChangePasswordForm from './ChangePasswordForm.vue'
import PasswordChangePage from './PasswordChangePage.vue'
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
    created_at: 1_700_000_000,
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

const server = vi.hoisted(() => ({ back: true }))
vi.mock('@/shared/lib/passphrase', async (original) => ({
  ...(await original<typeof import('@/shared/lib/passphrase')>()),
  waitForServer: () => Promise.resolve(server.back),
}))

beforeEach(() => {
  cleanup()
  server.back = true
})

/** Chooses `content` as the file of the input `selector`. */
async function chooseFile(selector: string, content: string): Promise<void> {
  const input = document.body.querySelector<HTMLInputElement>(selector)!
  const file = new File([content], 'zendiloos-config-nas-20261112T030000Z.tar.zst.age')
  Object.defineProperty(input, 'files', { value: [file], configurable: true })
  input.dispatchEvent(new Event('change'))
  await settle()
}

describe('restoring instead of the setup', () => {
  const restored = {
    host_name: 'nas',
    created_at: 1_800_000_000,
    version: '0.4.0',
    warnings: ['SSH’s settings are not put back'],
    https_port: 443,
    network: [],
  }

  it('sends the backup with its passphrase and the code, then leads to the sign-in', async () => {
    on('POST', '/api/v1/setup/restore', () => restored)
    const router = memoryRouter()
    await router.push('/setup')
    mountApp(SetupPage, { global: { plugins: [router] } })
    await settle()
    document.body.querySelector<HTMLButtonElement>('[data-testid="restore-instead"]')!.click()
    await settle()
    await type('#setup-code', 'ABCD-EFGH-IJKL')
    await chooseFile('#setup-backup', 'encrypted')
    await type('#setup-passphrase', 'correct horse battery')
    await submit('#setup-passphrase')

    const call = callsTo('POST', '/api/v1/setup/restore')[0]!
    expect(call.body).toBe('encrypted')
    expect(call.headers.get('x-zendiloos-passphrase')).toBe(btoa('correct horse battery'))
    expect(call.headers.get('x-zendiloos-setup-code')).toBe('ABCD-EFGH-IJKL')
    expect(page()).toContain('The backup of nas')
    expect(page()).toContain('SSH’s settings are not put back')
    expect(router.currentRoute.value.name).toBe('login')
  })

  it('says what is wrong with the backup, and when the restart takes long', async () => {
    on('GET', '/api/v1/setup', () => ({ required: true, code_required: false }))
    on('POST', '/api/v1/setup/restore', () => problem(422, 'wrong_passphrase'))
    mountApp(SetupPage, { global: { plugins: [memoryRouter()] } })
    await settle()
    document.body.querySelector<HTMLButtonElement>('[data-testid="restore-instead"]')!.click()
    await settle()
    await chooseFile('#setup-backup', 'encrypted')
    await type('#setup-passphrase', 'not the one')
    await submit('#setup-passphrase')
    expect(page()).toContain('The passphrase does not open this backup.')
    expect(
      callsTo('POST', '/api/v1/setup/restore')[0]!.headers.get('x-zendiloos-setup-code'),
    ).toBeNull()

    server.back = false
    on('POST', '/api/v1/setup/restore', () => restored)
    await submit('#setup-passphrase')
    expect(page()).toContain('takes longer than usual')
    expect(page()).toContain('Go to sign-in')
  })

  it('goes back to creating an administrator', async () => {
    mountApp(SetupPage, { global: { plugins: [memoryRouter()] } })
    await settle()
    document.body.querySelector<HTMLButtonElement>('[data-testid="restore-instead"]')!.click()
    await settle()
    await click('Create an administrator instead')
    expect(document.body.querySelector('#setup-name')).not.toBeNull()
  })
})

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

  it('asks for no code when the setup is open, and says who becomes the administrator', async () => {
    on('GET', '/api/v1/setup', () => ({ required: true, code_required: false }))
    on('POST', '/api/v1/setup', () => session)
    const router = memoryRouter()
    await router.push('/setup')
    mountApp(SetupPage, { global: { plugins: [router] } })
    await settle()

    expect(document.body.querySelector('#setup-code')).toBeNull()
    expect(page()).toContain('Whoever sets up the NAS first becomes its administrator.')
    await type('#setup-name', 'admin')
    await type('#setup-password', 'correct horse battery')
    await type('#setup-password-repeat', 'correct horse battery')
    await submit('#setup-name')

    expect(callsTo('POST', '/api/v1/setup')[0]!.body).toEqual({
      name: 'admin',
      password: 'correct horse battery',
    })
    expect(router.currentRoute.value.name).toBe('desktop')
  })

  it('says to set it up from the own network when it is opened from elsewhere', async () => {
    on('GET', '/api/v1/setup', () => ({ required: true, code_required: false }))
    on('POST', '/api/v1/setup', () => problem(403, 'setup_not_local'))
    mountApp(SetupPage, { global: { plugins: [memoryRouter()] } })
    await settle()
    await type('#setup-name', 'admin')
    await type('#setup-password', 'correct horse battery')
    await type('#setup-password-repeat', 'correct horse battery')
    await submit('#setup-name')
    expect(page()).toContain('Set up the NAS from your own network')
  })

  it('asks for the code when the setup says so', async () => {
    on('GET', '/api/v1/setup', () => ({ required: true, code_required: true }))
    mountApp(SetupPage, { global: { plugins: [memoryRouter()] } })
    await settle()
    expect(document.body.querySelector('#setup-code')).not.toBeNull()
    expect(page()).not.toContain('Whoever sets up the NAS first')
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

describe('choosing a password before going on', () => {
  async function choose(answerSession: () => unknown) {
    on('POST', '/api/v1/auth/password', () => undefined)
    on('GET', '/api/v1/auth/session', answerSession)
    const router = memoryRouter()
    router.addRoute({ path: '/password', name: 'password', component: PasswordChangePage })
    await router.push('/password')
    mountApp(PasswordChangePage, { global: { plugins: [router] } })
    await settle()
    await type('#password-current', 'set by an admin')
    await type('#password-new', 'my very own password')
    await type('#password-repeat', 'my very own password')
    await submit('#password-new')
    return router
  }

  it('goes to the desktop with the session as it is now', async () => {
    const router = await choose(() => session)
    expect(useSessionStore().info?.account.name).toBe('admin')
    expect(router.currentRoute.value.name).toBe('desktop')
  })

  it('forgets the session when the server has none any more', async () => {
    const router = await choose(() => problem(401, 'unauthenticated'))
    expect(useSessionStore().signedIn).toBe(false)
    expect(router.currentRoute.value.name).toBe('desktop')
  })
})
