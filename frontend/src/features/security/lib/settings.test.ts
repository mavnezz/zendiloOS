import { describe, expect, it } from 'vitest'

import type { SecuritySettings } from '@/shared/api/types'

import { fromDraft, invalid, sameSettings, toDraft } from './settings'

const DEFAULTS: SecuritySettings = {
  idle_minutes: 30,
  account_lock: { failures: 5, window_minutes: 15 },
  auto_block: { failures: 10, window_minutes: 5, release_after_days: 1 },
  allowed: ['192.168.1.0/24'],
}

describe('the security settings', () => {
  it('come back as they went in', () => {
    const draft = toDraft(DEFAULTS, 'tls1_2')
    expect(fromDraft(draft, DEFAULTS.allowed ?? [])).toEqual(DEFAULTS)
    expect(invalid(draft).size).toBe(0)
  })

  it('keep sensible numbers for what is turned off', () => {
    const off = toDraft({ ...DEFAULTS, account_lock: null, auto_block: null }, 'tls1_3')
    expect([off.lock, off.lock_failures, off.block, off.block_failures]).toEqual([
      false,
      '5',
      false,
      '10',
    ])
    expect(fromDraft(off, [])).toMatchObject({ account_lock: null, auto_block: null })
    const byHand = toDraft(
      { ...DEFAULTS, auto_block: { failures: 6, window_minutes: 10, release_after_days: null } },
      'tls1_2',
    )
    expect([byHand.release, byHand.release_days]).toEqual([false, '1'])
    expect(fromDraft(byHand, []).auto_block?.release_after_days).toBeNull()
  })

  it('check only what is turned on', () => {
    const draft = toDraft(DEFAULTS, 'tls1_2')
    expect([...invalid({ ...draft, idle_minutes: '4', lock_failures: '2' })]).toEqual([
      'idle_minutes',
      'lock_failures',
    ])
    expect(invalid({ ...draft, lock: false, lock_failures: '2' }).size).toBe(0)
    expect([...invalid({ ...draft, release_days: '0' })]).toEqual(['release_days'])
    expect(invalid({ ...draft, release: false, release_days: '0' }).size).toBe(0)
    expect([...invalid({ ...draft, block_window: '1.5' })]).toEqual(['block_window'])
  })

  it('see spaces and the TLS version as no change to the settings', () => {
    const draft = toDraft(DEFAULTS, 'tls1_2')
    expect(sameSettings(draft, { ...draft, idle_minutes: ' 30 ', tls: 'tls1_3' })).toBe(true)
    expect(sameSettings(draft, { ...draft, lock: false })).toBe(false)
  })
})
