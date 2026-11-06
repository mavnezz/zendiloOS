import { describe, expect, it } from 'vitest'

import type { SshState } from '@/shared/api/types'

import { fromDraft, portValid, sameSsh, toDraft } from './ssh'

const NOW = 1_800_000_000

function state(extra: Partial<SshState> = {}): SshState {
  return {
    installed: true,
    enabled: true,
    running: true,
    port: 22,
    root_login: null,
    password_login: null,
    off_at: null,
    ...extra,
  }
}

describe('the SSH settings', () => {
  it('take 22 or a free port from 1024', () => {
    for (const port of ['22', '1024', ' 2222 ', '65535']) expect(portValid(port), port).toBe(true)
    for (const port of ['', '0', '23', '445', '1023', '2049', '5353', '65536', '22a', '-22'])
      expect(portValid(port), port).toBe(false)
  })

  it('leave the rules to the system unless one is chosen', () => {
    const draft = toDraft(state())
    expect(draft).toEqual({
      enabled: true,
      port: '22',
      root_login: 'system',
      password_login: 'system',
      off_after: 'never',
    })
    expect(fromDraft(draft, null, NOW)).toEqual({
      enabled: true,
      port: 22,
      root_login: null,
      password_login: null,
      off_after_minutes: null,
    })
    const chosen = { ...draft, root_login: 'key_only' as const, password_login: 'no' as const }
    expect(fromDraft(chosen, null, NOW)).toMatchObject({
      root_login: 'key_only',
      password_login: false,
    })
  })

  it('keep a planned turn-off at its time, and plan none while off', () => {
    const planned = toDraft(state({ off_at: NOW + 1_250 }))
    expect(planned.off_after).toBe('planned')
    expect(fromDraft(planned, NOW + 1_250, NOW).off_after_minutes).toBe(21)
    expect(fromDraft(planned, NOW - 5, NOW).off_after_minutes, 'already due').toBe(1)
    expect(fromDraft({ ...planned, off_after: '30' }, NOW + 1_250, NOW).off_after_minutes).toBe(30)
    expect(fromDraft({ ...planned, enabled: false }, NOW + 1_250, NOW).off_after_minutes).toBeNull()
    expect(toDraft(state({ enabled: false, running: false })).off_after).toBe('never')
  })

  it('see a port with spaces as the same port', () => {
    const draft = toDraft(state())
    expect(sameSsh(draft, { ...draft, port: ' 22 ' })).toBe(true)
    expect(sameSsh(draft, { ...draft, password_login: 'yes' })).toBe(false)
  })
})
