import {
  answer,
  callsTo,
  cleanup,
  click,
  mountApp,
  on,
  page,
  press,
  problem,
  settle,
  type,
} from '@/shared/testing/server'

import { beforeEach, describe, expect, it, vi } from 'vitest'

import { useNotifications } from '@/shared/lib/notifications'

import { useSessionStore } from '../store'
import TwoFactorDialog from './TwoFactorDialog.vue'

const SECRET = 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ'
const CODES = ['abcde-fghjk', 'mnpqr-stuvw']

async function show() {
  const wrapper = mountApp(TwoFactorDialog, { props: { open: true } })
  useSessionStore().start({
    account: {
      id: 1,
      name: 'alice',
      full_name: null,
      role: 'user',
      permissions: [],
      password_change_required: false,
      password_locked: false,
    },
    csrf_token: 'csrf-1',
    expires_at: 0,
  })
  await settle()
  return wrapper
}

const notified = (title: string) => useNotifications().items.some((n) => n.title === title)

beforeEach(() => {
  cleanup()
})

describe('two-factor sign-in', () => {
  it('is set up with a QR code and its first code, and shows the recovery codes once', async () => {
    answer('GET', '/api/v1/auth/two-factor', { enabled: false, recovery_codes_left: 0 })
    on('POST', '/api/v1/auth/two-factor/setup', () => ({
      secret: SECRET,
      uri: `otpauth://totp/zendiloOS:alice?secret=${SECRET}&issuer=zendiloOS`,
    }))
    let tries = 0
    on('POST', '/api/v1/auth/two-factor/confirm', () =>
      ++tries === 1 ? problem(422, 'invalid_code') : { recovery_codes: CODES },
    )
    await show()
    expect(page()).toContain('Your password alone signs you in.')

    await click('Set up')

    expect(document.querySelector('svg[role="img"]')?.getAttribute('aria-label')).toBe(
      'QR code for the authenticator app',
    )
    expect(document.querySelector('svg[role="img"] path')?.getAttribute('d')).toMatch(/^M\d/)
    expect(document.querySelector('[data-testid="key"]')?.textContent?.trim()).toBe(
      'GEZD GNBV GY3T QOJQ GEZD GNBV GY3T QOJQ',
    )
    const code = document.querySelector<HTMLInputElement>('#two-factor-code')!
    expect(code.autocomplete).toBe('one-time-code')
    await type('#two-factor-code', '000000')
    await click('Turn on')
    expect(page()).toContain('The code is wrong.')
    await type('#two-factor-code', '123456')
    await click('Turn on')

    expect(callsTo('POST', '/api/v1/auth/two-factor/confirm')[1]!.body).toEqual({ code: '123456' })
    const shown = [...document.querySelectorAll('[data-testid="recovery-codes"] li')]
    expect(shown.map((li) => li.textContent)).toEqual(CODES)
    expect(notified('Two-factor sign-in is on')).toBe(true)
    answer('GET', '/api/v1/auth/two-factor', { enabled: true, recovery_codes_left: 2 })
    await click('I have saved them')
    expect(page()).not.toContain('abcde-fghjk')
    expect(document.querySelector('[data-testid="codes-left"]')?.textContent?.trim()).toBe(
      '2 recovery codes left',
    )
  })

  it('copies the key and the recovery codes and downloads them', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    const createObjectURL = vi.fn().mockReturnValue('blob:codes')
    Object.assign(URL, { createObjectURL, revokeObjectURL: vi.fn() })
    answer('GET', '/api/v1/auth/two-factor', { enabled: false, recovery_codes_left: 0 })
    answer('POST', '/api/v1/auth/two-factor/setup', { secret: SECRET, uri: 'otpauth://totp/x' })
    answer('POST', '/api/v1/auth/two-factor/confirm', { recovery_codes: CODES })
    await show()
    await click('Set up')

    await press('Copy key')
    expect(writeText).toHaveBeenLastCalledWith(SECRET)
    await type('#two-factor-code', '123456')
    await click('Turn on')
    await click('Copy')
    expect(writeText).toHaveBeenLastCalledWith('abcde-fghjk\nmnpqr-stuvw')
    expect(notified('Recovery codes copied')).toBe(true)
    await click('Download')

    const blob = createObjectURL.mock.calls[0]![0] as Blob
    expect(await blob.text()).toContain('alice')
    expect(await blob.text()).toContain('abcde-fghjk\nmnpqr-stuvw')
  })

  it('makes new recovery codes for a code from the app', async () => {
    answer('GET', '/api/v1/auth/two-factor', { enabled: true, recovery_codes_left: 1 })
    answer('POST', '/api/v1/auth/two-factor/recovery-codes', { recovery_codes: CODES })
    await show()
    expect(page()).toContain('One recovery code left')

    await click('New recovery codes')
    await type('#two-factor-code', '654321')
    await click('Make new codes')

    expect(callsTo('POST', '/api/v1/auth/two-factor/recovery-codes')[0]!.body).toEqual({
      code: '654321',
    })
    expect(page()).toContain('mnpqr-stuvw')
  })

  it('is turned off with the password and a code', async () => {
    answer('GET', '/api/v1/auth/two-factor', { enabled: true, recovery_codes_left: 10 })
    let tries = 0
    on('POST', '/api/v1/auth/two-factor/disable', () =>
      ++tries === 1 ? problem(422, 'wrong_password') : undefined,
    )
    await show()

    await click('Turn off')
    await type('#two-factor-password', 'wrong')
    await type('#two-factor-code', 'abcde-fghjk')
    await click('Turn off two-factor sign-in')
    expect(page()).toContain('The current password is wrong.')
    answer('GET', '/api/v1/auth/two-factor', { enabled: false, recovery_codes_left: 0 })
    await type('#two-factor-password', 'correct horse battery')
    await type('#two-factor-code', '123456')
    await click('Turn off two-factor sign-in')

    expect(callsTo('POST', '/api/v1/auth/two-factor/disable')[1]!.body).toEqual({
      password: 'correct horse battery',
      code: '123456',
    })
    expect(notified('Two-factor sign-in is off')).toBe(true)
    expect(page()).toContain('Your password alone signs you in.')
  })

  it('goes back from a form without changing anything', async () => {
    answer('GET', '/api/v1/auth/two-factor', { enabled: true, recovery_codes_left: 10 })
    await show()

    await click('Turn off')
    await click('Back')

    expect(page()).toContain('10 recovery codes left')
    expect(callsTo('POST', '/api/v1/auth/two-factor/disable')).toHaveLength(0)
  })

  it('says when the status cannot be loaded', async () => {
    on('GET', '/api/v1/auth/two-factor', () => problem(500, 'internal'))

    await show()

    expect(document.querySelector('[role="alert"]')).not.toBeNull()
  })
})
