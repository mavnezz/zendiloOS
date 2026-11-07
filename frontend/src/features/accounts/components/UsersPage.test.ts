import {
  answer,
  callsTo,
  cleanup,
  click,
  mountApp,
  on,
  page,
  pick,
  problem,
  settle,
  type,
} from '@/shared/testing/server'

import { beforeEach, describe, expect, it } from 'vitest'

import { useNotifications } from '@/shared/lib/notifications'
import { account, group, homes, share } from '@/shared/testing/fixtures'

import AccountRules from './AccountRules.vue'
import UsersPage from './UsersPage.vue'

const me = account(1, 'admin', { role: 'admin', full_name: 'The Admin' })
const anna = account(5, 'anna', { email: 'anna@example.org', description: 'Photos' })
const ben = account(6, 'ben', { disabled: true })
const cara = account(7, 'cara', { password_change_required: true })

function serve(): void {
  answer('GET', '/api/v1/accounts', [me, anna, ben, cara])
  answer('GET', '/api/v1/groups', [
    group(1, 'family', [5, 6, 99]),
    group(2, 'empty', [], { description: 'Spare' }),
  ])
  answer('GET', '/api/v1/shares', [share(10, 'photos')])
  answer('GET', '/api/v1/homes', homes(false))
  answer('GET', '/api/v1/password-policy', {
    min_length: 12,
    digit: false,
    mixed_case: true,
    symbol: false,
  })
  answer('GET', '/api/v1/storage/pools', [{ name: 'tank' }, { name: 'cold' }])
}

async function show() {
  const wrapper = mountApp(UsersPage, { props: { ownAccountId: 1 } })
  await settle()
  return wrapper
}

beforeEach(() => {
  cleanup()
  serve()
})

describe('the users', () => {
  it('lists every account with its role and state, and filters them', async () => {
    await show()

    expect(page()).toContain('The Admin')
    expect(page()).toContain('Me')
    expect(page()).toContain('Administrator')
    expect(page()).toContain('Disabled')
    expect(page()).toContain('Must change password')
    expect(page()).toContain('Normal')
    await type('input[aria-label="Filter"]', 'example.org')
    expect(page()).toContain('anna')
    expect(page()).not.toContain('cara')
    await type('input[aria-label="Filter"]', 'nobody')
    expect(page()).toContain('Nothing matches the filter.')
  })

  it('turns an account off and on, and says when that fails', async () => {
    on('PATCH', '/api/v1/accounts/5', () => ({ ...anna, disabled: true }))
    on('PATCH', '/api/v1/accounts/6', () => problem(409, 'last_admin'))
    await show()

    await pick('Actions for anna', 'Disable')
    expect(callsTo('PATCH', '/api/v1/accounts/5')[0]!.body).toEqual({ disabled: true })
    await pick('Actions for ben', 'Enable')
    expect(callsTo('PATCH', '/api/v1/accounts/6')[0]!.body).toEqual({ disabled: false })
    expect(
      useNotifications().items.some((n) => n.title === 'The account could not be changed'),
    ).toBe(true)
  })

  it('sets a new password', async () => {
    on('POST', '/api/v1/accounts/5/password', () => undefined)
    await show()

    await pick('Actions for anna', 'Set password')
    expect(page()).toContain('Set a password for anna')
    await type('#reset-password', 'correct horse battery')
    document.body.querySelector<HTMLFormElement>('#reset-password')!.form!.requestSubmit()
    await settle()
    expect(callsTo('POST', '/api/v1/accounts/5/password')[0]!.body).toEqual({
      password: 'correct horse battery',
    })
    expect(page()).not.toContain('Set a password for anna')
  })

  it('shows why a password was not set', async () => {
    on('POST', '/api/v1/accounts/5/password', () => problem(422, 'invalid_input', 'too common'))
    await show()
    await pick('Actions for anna', 'Set password')
    await type('#reset-password', 'password123')
    document.body.querySelector<HTMLFormElement>('#reset-password')!.form!.requestSubmit()
    await settle()
    expect(page()).toContain('too common')
    await click('Cancel')
    expect(page()).not.toContain('Set a password for anna')
  })

  it('deletes an account once confirmed', async () => {
    on('DELETE', '/api/v1/accounts/5', () => undefined)
    on('DELETE', '/api/v1/accounts/6', () => problem(500, 'internal'))
    await show()

    await pick('Actions for anna', 'Delete')
    expect(page()).toContain('Delete anna?')
    await click('Delete')
    expect(callsTo('DELETE', '/api/v1/accounts/5')).toHaveLength(1)
    expect(useNotifications().items.some((n) => n.title === 'anna was deleted')).toBe(true)
    await pick('Actions for ben', 'Delete')
    await click('Delete')
    expect(useNotifications().items.some((n) => n.title === 'The user could not be deleted')).toBe(
      true,
    )
  })

  it('opens the wizard to create and to edit', async () => {
    await show()
    await click('Add')
    expect(page()).toContain('Create user')
    await click('Cancel')
    await pick('Actions for anna', 'Edit')
    expect(page()).toContain('Edit anna')
  })

  it('says when the list cannot be read, and when there is nobody', async () => {
    on('GET', '/api/v1/accounts', () => problem(500, 'internal'))
    await show()
    expect(document.body.querySelector('[role="alert"]')).not.toBeNull()

    cleanup()
    serve()
    answer('GET', '/api/v1/accounts', [])
    await show()
    expect(page()).toContain('No users.')
  })
})

describe('the groups', () => {
  it('lists groups with their members, filters, edits and deletes them', async () => {
    on('DELETE', '/api/v1/groups/2', () => undefined)
    on('DELETE', '/api/v1/groups/1', () => problem(500, 'internal'))
    await show()
    await click('Groups')

    expect(page()).toContain('family')
    expect(page()).toContain('No members.')
    expect(document.body.querySelector('[title="anna, ben, #99"]')).not.toBeNull()
    await type('input[aria-label="Filter"]', 'spare')
    expect(page()).not.toContain('family')
    await type('input[aria-label="Filter"]', 'zzz')
    expect(page()).toContain('Nothing matches the filter.')
    await type('input[aria-label="Filter"]', '')

    await pick('Actions for empty', 'Delete')
    expect(page()).toContain('Delete group empty?')
    await click('Delete')
    expect(callsTo('DELETE', '/api/v1/groups/2')).toHaveLength(1)
    await pick('Actions for family', 'Delete')
    await click('Delete')
    expect(useNotifications().items.some((n) => n.title === 'The group could not be deleted')).toBe(
      true,
    )

    await pick('Actions for family', 'Edit')
    expect(page()).toContain('Edit family')
    await click('Cancel')
    await click('Create group')
    expect(page()).toContain('Group info')
  })

  it('says when there are none, and when they cannot be read', async () => {
    answer('GET', '/api/v1/groups', [])
    await show()
    await click('Groups')
    expect(page()).toContain('No groups yet.')

    cleanup()
    serve()
    on('GET', '/api/v1/groups', () => problem(500, 'internal'))
    await show()
    await click('Groups')
    expect(document.body.querySelector('[role="alert"]')).not.toBeNull()
  })
})

describe('the rules for every account', () => {
  it('changes the password rules and turns personal folders on', async () => {
    on('PUT', '/api/v1/password-policy', ({ body }) => body)
    on('PUT', '/api/v1/homes', () => homes(true))
    await show()
    await click('Advanced')

    expect(document.body.querySelector<HTMLInputElement>('#policy-length')!.value).toBe('12')
    await type('#policy-length', '14')
    document.body.querySelector<HTMLElement>('#policy-expires')!.click()
    await settle()
    await type('#policy-age', '60')
    document.body.querySelector<HTMLElement>('#homes-on')!.click()
    await settle()
    const pool = document.body.querySelector<HTMLSelectElement>('#homes-pool')!
    pool.value = 'cold'
    pool.dispatchEvent(new Event('change'))
    await settle()
    await click('Apply')

    expect(callsTo('PUT', '/api/v1/password-policy')[0]!.body).toEqual({
      min_length: 14,
      mixed_case: true,
      digit: false,
      symbol: false,
      max_age_days: 60,
    })
    expect(callsTo('PUT', '/api/v1/homes')[0]!.body).toEqual({ pool: 'cold' })
  })

  it('takes nothing out of range, resets, and shows a refusal', async () => {
    on('PUT', '/api/v1/password-policy', () => problem(422, 'invalid_input', 'no'))
    mountApp(AccountRules)
    await settle()
    await type('#policy-length', '8')
    expect(buttons('Apply')[0]!.disabled).toBe(true)
    await click('Reset')
    expect(document.body.querySelector<HTMLInputElement>('#policy-length')!.value).toBe('12')
    await type('#policy-length', '16')
    await click('Apply')
    expect(page()).toContain('no')
  })
})

function buttons(text: string): HTMLButtonElement[] {
  return [...document.body.querySelectorAll('button')].filter((b) => b.textContent?.trim() === text)
}
