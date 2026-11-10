import { describe, expect, it } from 'vitest'

import { estimatePool, fits, fittingLayout, misfit, protectionOf } from './poolEstimate'

const GB = 1e9

describe('estimatePool', () => {
  it('gives a single disk all its space and no protection', () => {
    expect(estimatePool('single', [4 * GB])).toEqual({
      usable: 4 * GB,
      protection: 0,
      unused: 0,
      read: 1,
      write: 1,
      tolerates: 0,
    })
  })

  it('keeps one disk of a mirror for data, up to the smallest', () => {
    expect(estimatePool('mirror', [20 * GB, 32 * GB, 32 * GB])).toEqual({
      usable: 20 * GB,
      protection: 40 * GB,
      unused: 24 * GB,
      read: 3,
      write: 1,
      tolerates: 2,
    })
  })

  it('gives RAIDZ parity as many disks as it tolerates failing', () => {
    const sizes = [32 * GB, 20 * GB, 32 * GB, 32 * GB]
    expect(estimatePool('raidz1', sizes)).toEqual({
      usable: 60 * GB,
      protection: 20 * GB,
      unused: 36 * GB,
      read: 3,
      write: 3,
      tolerates: 1,
    })
    expect(estimatePool('raidz2', sizes)).toMatchObject({
      usable: 40 * GB,
      protection: 40 * GB,
      read: 2,
      write: 2,
      tolerates: 2,
    })
    expect(estimatePool('raidz3', [...sizes, 20 * GB])).toMatchObject({
      usable: 40 * GB,
      protection: 60 * GB,
      unused: 36 * GB,
      tolerates: 3,
    })
  })

  it('has no estimate for disks the layout does not take', () => {
    expect(estimatePool('mirror', [])).toBeNull()
    expect(estimatePool('single', [GB, GB])).toBeNull()
    expect(estimatePool('raidz2', [GB, GB, GB])).toBeNull()
  })
})

describe('layout limits', () => {
  it('says why disks do not fit', () => {
    expect(misfit('raidz1', 3)).toBeNull()
    expect(misfit('raidz1', 2)).toEqual({ minimum: 3, maximum: 12, reason: 'tooFew' })
    expect(misfit('mirror', 13)).toEqual({ minimum: 2, maximum: 12, reason: 'tooMany' })
    expect(misfit('single', 0)).toEqual({ minimum: 1, maximum: 1, reason: 'exactly' })
    expect(misfit('single', 2)?.reason).toBe('exactly')
    expect(fits('raidz3', 5)).toBe(true)
    expect(fits('raidz3', 4)).toBe(false)
  })

  it('keeps a fitting layout and otherwise takes the first that fits', () => {
    expect(fittingLayout('raidz1', 4)).toBe('raidz1')
    expect(fittingLayout('raidz2', 3)).toBe('mirror')
    expect(fittingLayout('mirror', 1)).toBe('single')
    expect(fittingLayout('raidz1', 0)).toBe('raidz1')
  })
})

describe('protectionOf', () => {
  it('rates the failures a layout survives', () => {
    expect(protectionOf(0)).toBe('none')
    expect(protectionOf(1)).toBe('medium')
    expect(protectionOf(2)).toBe('strong')
    expect(protectionOf(3)).toBe('very_strong')
    expect(protectionOf(11)).toBe('very_strong')
  })
})
