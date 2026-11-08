import { describe, expect, it } from 'vitest'

import { effectiveLevel, isNfsClient } from './permissions'

describe('what holds for an account', () => {
  it('lets a denial win, then the own rule, then the broadest group', () => {
    expect(effectiveLevel(undefined, ['read_write', 'read'])).toBe('read_write')
    expect(effectiveLevel(undefined, ['read', undefined])).toBe('read')
    expect(effectiveLevel('read', ['read_write'])).toBe('read')
    expect(effectiveLevel('read_write', ['deny'])).toBe('deny')
    expect(effectiveLevel('deny', ['read_write'])).toBe('deny')
    expect(effectiveLevel('none', ['none'])).toBeNull()
    expect(effectiveLevel(undefined, [])).toBeNull()
  })
})

describe('an NFS client', () => {
  it('is an address, a network, a host name or a pattern', () => {
    for (const good of [
      '192.168.1.0/24',
      '10.0.0.5',
      'fd00::/64',
      'nas.local',
      'NAS.lan',
      '*.example.com',
      'host?.lan',
      '*',
    ]) {
      expect(isNfsClient(good), good).toBe(true)
    }
    for (const bad of [
      '',
      'nas lan',
      'nas(rw)',
      'a,b',
      '-a.lan',
      'a..lan',
      '10.0.0.0/33',
      '300.1.1.1',
      '@group',
      '/volume1',
      `${'a'.repeat(250)}.lan`,
    ]) {
      expect(isNfsClient(bad), bad).toBe(false)
    }
  })
})
