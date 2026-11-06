import { describe, expect, it } from 'vitest'

import { effectiveAccess } from './access'

const share = (
  access: { kind: 'account' | 'group'; id: number; access: 'deny' | 'read' | 'read_write' }[],
) => ({
  access,
})

describe('what an account may do in a share', () => {
  it('follows the broadest grant of its groups', () => {
    const photos = share([
      { kind: 'group', id: 10, access: 'read_write' },
      { kind: 'group', id: 20, access: 'read' },
    ])
    expect(effectiveAccess(photos, 1, [10, 20])).toBe('read_write')
    expect(effectiveAccess(photos, 1, [20])).toBe('read')
    expect(effectiveAccess(photos, 1, [])).toBeNull()
  })

  it('takes its own rule over its groups, and a denial over everything', () => {
    const photos = share([
      { kind: 'group', id: 10, access: 'read_write' },
      { kind: 'account', id: 1, access: 'read' },
    ])
    expect(effectiveAccess(photos, 1, [10])).toBe('read')
    expect(effectiveAccess(photos, 1, [10], 'deny')).toBeNull()
    expect(effectiveAccess(photos, 1, [10], null)).toBe('read_write')
    const kidsDenied = share([
      { kind: 'account', id: 1, access: 'read_write' },
      { kind: 'group', id: 30, access: 'deny' },
    ])
    expect(effectiveAccess(kidsDenied, 1, [30])).toBeNull()
  })
})
