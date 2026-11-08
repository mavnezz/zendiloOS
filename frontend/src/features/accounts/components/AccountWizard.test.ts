import {
  answer,
  callsTo,
  cleanup,
  click,
  mountApp,
  on,
  problem,
  settle,
  type,
} from '@/shared/testing/server'

import { beforeEach, describe, expect, it } from 'vitest'

import { useNotifications } from '@/shared/lib/notifications'
import { account, group, homes, share } from '@/shared/testing/fixtures'

import AccountWizard from './AccountWizard.vue'

const anna = account(5, 'anna', { full_name: 'Anna A', email: 'anna@example.org' })

function serve(): void {
  answer('GET', '/api/v1/groups', [group(1, 'family', [5]), group(2, 'staff', [])])
  answer('GET', '/api/v1/shares', [
    share(10, 'photos', { access: [{ kind: 'account', id: 5, access: 'read' }] }),
    share(11, 'media'),
  ])
  answer(
    'GET',
    '/api/v1/homes',
    homes(true, { homes: [{ account_id: 5, quota_bytes: 2 * 1024 ** 4 }] }),
  )
  answer('GET', '/api/v1/password-policy', {
    min_length: 10,
    digit: false,
    mixed_case: false,
    symbol: false,
  })
  answer('GET', '/api/v1/accounts', [anna])
}

function input(selector: string): HTMLInputElement {
  return document.body.querySelector<HTMLInputElement>(selector)!
}

function check(label: string, on = true): Promise<void> {
  const box = [...document.body.querySelectorAll<HTMLLabelElement>('label')].find((l) =>
    l.textContent?.includes(label),
  )
  const target = box?.querySelector<HTMLInputElement>('input') ?? null
  if (!target) throw new Error(`no ${label}`)
  if (target.checked !== on) target.click()
  return settle()
}

async function open(props: Record<string, unknown> = {}) {
  const wrapper = mountApp(AccountWizard, { props: { open: true, ...props } })
  await settle()
  return wrapper
}

beforeEach(() => {
  cleanup()
  serve()
})

describe('creating an account', () => {
  it('asks for a name and a long enough, matching password before the next step', async () => {
    await open()

    await click('Next')
    expect(document.body.textContent).toContain('Enter a name.')
    await type('#account-name', 'ben')
    await type('#account-password', 'short')
    await click('Next')
    expect(document.body.textContent).toContain('The password needs at least 10 characters.')
    await type('#account-password', 'correct horse battery')
    await type('#account-confirm', 'correct horse')
    await click('Next')
    expect(document.body.textContent).toContain('The passwords do not match.')
  })

  it('creates the account, then its groups, personal folder and share rules', async () => {
    on('POST', '/api/v1/accounts', () => account(6, 'ben'))
    on('PATCH', '/api/v1/groups/2', ({ body }) => ({
      ...group(2, 'staff', (body as { members: number[] }).members),
      pending_shares: [],
    }))
    on('PUT', '/api/v1/homes/6', () => ({ account_id: 6, quota_bytes: 5 * 1024 ** 3 }))
    on('PUT', '/api/v1/shares/access/account/6', () => ({ jobs: [] }))
    const wrapper = await open()

    await type('#account-name', ' ben ')
    await click('Generate')
    expect(input('#account-password').value.length).toBeGreaterThanOrEqual(16)
    expect(input('#account-password').type).toBe('text')
    await type('#account-full-name', 'Ben B')
    await check('staff')
    await type('input[type="number"]', '5')
    await click('Next')
    // The permissions: read and write on media.
    document.body.querySelector<HTMLInputElement>('input[aria-label="media: Read/write"]')!.click()
    await settle()
    await click('Done')

    const created = callsTo('POST', '/api/v1/accounts')[0]!.body as Record<string, unknown>
    expect(created).toMatchObject({
      name: 'ben',
      full_name: 'Ben B',
      role: 'user',
      password_locked: false,
    })
    expect(callsTo('PATCH', '/api/v1/groups/2')[0]!.body).toEqual({ members: [6] })
    expect(callsTo('PUT', '/api/v1/homes/6')[0]!.body).toEqual({
      enabled: true,
      quota_bytes: 5 * 1024 ** 3,
    })
    expect(callsTo('PUT', '/api/v1/shares/access/account/6')[0]!.body).toEqual({
      rules: [{ share_id: 11, rule: 'read_write' }],
    })
    expect(wrapper.emitted('update:open')?.at(-1)).toEqual([false])
  })

  it('goes back to the first step and refuses a quota below a gigabyte', async () => {
    await open()
    await type('#account-name', 'ben')
    await type('#account-password', 'correct horse battery')
    await type('#account-confirm', 'correct horse battery')
    await click('Next')
    await click('Back')
    expect(input('#account-name').value).toBe('ben')
    await type('input[type="number"]', '0.5')
    await click('Next')
    expect(document.body.textContent).toContain('A quota is at least 1 GB.')
  })

  it('says the account exists when what follows fails, and shows other failures in place', async () => {
    on('POST', '/api/v1/accounts', () => problem(409, 'name_taken'))
    await open()
    await type('#account-name', 'ben')
    await type('#account-password', 'correct horse battery')
    await type('#account-confirm', 'correct horse battery')
    await click('Next')
    await click('Done')
    expect(
      document.body.querySelector('[role="alert"]')?.textContent ?? document.body.textContent,
    ).toMatch(/taken|exists/i)

    cleanup()
    serve()
    on('POST', '/api/v1/accounts', () => account(6, 'ben'))
    on('PUT', '/api/v1/homes/6', () => problem(500, 'internal'))
    const wrapper = await open()
    await type('#account-name', 'ben')
    await type('#account-password', 'correct horse battery')
    await type('#account-confirm', 'correct horse battery')
    await click('Next')
    await click('Done')
    expect(wrapper.emitted('update:open')?.at(-1)).toEqual([false])
  })

  it('shows the password on request', async () => {
    await open()
    const toggle = document.body.querySelector<HTMLButtonElement>(
      'button[aria-label="Show password"]',
    )!
    toggle.click()
    await settle()
    expect(input('#account-password').type).toBe('text')
    document.body.querySelector<HTMLButtonElement>('button[aria-label="Hide password"]')!.click()
    await settle()
    expect(input('#account-password').type).toBe('password')
  })
})

describe('editing an account', () => {
  it('fills the form from the account, and saves only what changed after it', async () => {
    on('PATCH', '/api/v1/accounts/5', () => ({ ...anna, pending_shares: [] }))
    on('PATCH', '/api/v1/groups/1', () => ({
      ...group(1, 'family', []),
      pending_shares: ['photos'],
    }))
    on('PUT', '/api/v1/homes/5', () => null)
    const wrapper = await open({ account: anna })

    expect(input('#account-name').disabled).toBe(true)
    expect(input('#account-full-name').value).toBe('Anna A')
    expect(input('input[type="number"]').value).toBe('2')
    await check('family', false)
    await check('Personal folder', false)
    await type('#account-description', 'Photographer')
    await click('Save')

    expect(callsTo('PATCH', '/api/v1/accounts/5')[0]!.body).toEqual({
      full_name: 'Anna A',
      description: 'Photographer',
      email: 'anna@example.org',
      role: 'user',
      disabled: false,
      password_locked: false,
    })
    expect(callsTo('PATCH', '/api/v1/groups/1')[0]!.body).toEqual({ members: [] })
    expect(callsTo('PUT', '/api/v1/homes/5')[0]!.body).toEqual({
      enabled: false,
      quota_bytes: null,
    })
    expect(callsTo('PUT', '/api/v1/shares/access/account/5')).toHaveLength(0)
    expect(wrapper.emitted('update:open')?.at(-1)).toEqual([false])
    // Leaving family: the files of photos follow in the background.
    expect(useNotifications().items[0]).toMatchObject({
      title: 'Changes to anna saved',
      body: 'The files in photos get their permissions in the background.',
    })
  })

  it('leaves role and locks alone for the own account, and shows the permissions tab', async () => {
    on('PATCH', '/api/v1/accounts/5', () => ({ ...anna, pending_shares: [] }))
    on('PUT', '/api/v1/shares/access/account/5', () => ({ jobs: [] }))
    await open({ account: anna, own: true })

    expect(input('#account-role').disabled).toBe(true)
    expect(document.body.textContent).toContain('Your own role and access cannot be changed here.')
    const tab = [...document.body.querySelectorAll<HTMLElement>('[role="tab"]')].find(
      (t) => t.textContent?.trim() === 'Permissions',
    )!
    tab.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
    tab.click()
    await settle()
    document.body
      .querySelector<HTMLInputElement>('input[aria-label="photos: Deny access"]')!
      .click()
    await settle()
    await click('Save')

    expect(callsTo('PATCH', '/api/v1/accounts/5')[0]!.body).toEqual({
      full_name: 'Anna A',
      description: '',
      email: 'anna@example.org',
    })
    expect(callsTo('PUT', '/api/v1/shares/access/account/5')[0]!.body).toEqual({
      rules: [{ share_id: 10, rule: 'deny' }],
    })
  })

  it('shows why saving failed', async () => {
    on('PATCH', '/api/v1/accounts/5', () => problem(409, 'last_admin'))
    await open({ account: anna })
    await click('Save')
    expect(document.body.querySelector('[role="alert"]')).not.toBeNull()
  })
})
