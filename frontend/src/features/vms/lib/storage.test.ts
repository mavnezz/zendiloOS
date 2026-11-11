import { describe, expect, it } from 'vitest'

import type { VmLocation, VmPlugin } from '@/shared/api/types'

import { freePools, poolFill, removalBlocked, vmShare } from './storage'

const GIB = 1024 ** 3

function location(extra: Partial<VmLocation> = {}): VmLocation {
  return {
    pool: 'tank',
    volume: 1,
    library: true,
    health: 'online',
    pool_used_bytes: 25 * GIB,
    available_bytes: 75 * GIB,
    used_bytes: 10 * GIB,
    images_bytes: GIB,
    images: 0,
    vms: [],
    ...extra,
  }
}

const vm = {
  id: '0a1b2c3d4e5f',
  name: 'web',
  disks: [0],
  used_bytes: 4 * GIB,
  referenced_bytes: 3 * GIB,
  size_bytes: 20 * GIB,
}

describe('the locations of the VMs’ storage', () => {
  it('say how full their pool is and what each VM takes of them', () => {
    expect(poolFill(location())).toBe(0.25)
    expect(poolFill(location({ pool_used_bytes: null }))).toBeNull()
    expect(poolFill(location({ pool_used_bytes: 0, available_bytes: 0 }))).toBeNull()
    expect(vmShare(vm, location())).toBe(0.4)
    expect(vmShare({ ...vm, used_bytes: null }, location())).toBeNull()
    expect(vmShare(vm, location({ used_bytes: 0 }))).toBeNull()
    expect(vmShare({ ...vm, used_bytes: 20 * GIB }, location())).toBe(1)
  })

  it('offer the pools that hold none yet, by volume', () => {
    const pool = (name: string, volume: number) => ({
      pool: name,
      volume,
      health: 'online',
      used_bytes: 0,
      available_bytes: 0,
    })
    const plugin = {
      locations: [{ pool: 'tank', volume: 1 }],
      pools: [pool('cold', 3), pool('tank', 1), pool('fast', 2)],
    } as unknown as VmPlugin
    expect(freePools(plugin).map((pool) => pool.pool)).toEqual(['fast', 'cold'])
    expect(freePools(undefined)).toEqual([])
  })

  it('go only once nothing lies on them, and the library’s last', () => {
    const second = location({ pool: 'cold', volume: 2, library: false })
    expect(removalBlocked(location({ vms: [vm] }), [location()])).toBe('vms')
    expect(removalBlocked(location({ images: 2 }), [location()])).toBe('images')
    expect(removalBlocked(location(), [location(), second])).toBe('library')
    expect(removalBlocked(second, [location(), second])).toBeNull()
    expect(removalBlocked(location(), [location()])).toBeNull()
  })
})
