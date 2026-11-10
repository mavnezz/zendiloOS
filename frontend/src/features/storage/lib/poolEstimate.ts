import type { NewPoolLayout } from '@/shared/api/types'

/** Most disks in one pool, as the backend allows (storage/domain/plan.rs). */
export const MAX_POOL_DISKS = 12

/** The layouts a new pool may have, in the order they are offered. */
export const POOL_LAYOUTS: NewPoolLayout[] = ['mirror', 'raidz1', 'raidz2', 'raidz3', 'single']

/** How many disks each layout takes; RAIDZ at least two more than its parity. */
const LIMITS: Record<NewPoolLayout, { minimum: number; maximum: number }> = {
  mirror: { minimum: 2, maximum: MAX_POOL_DISKS },
  raidz1: { minimum: 3, maximum: MAX_POOL_DISKS },
  raidz2: { minimum: 4, maximum: MAX_POOL_DISKS },
  raidz3: { minimum: 5, maximum: MAX_POOL_DISKS },
  single: { minimum: 1, maximum: 1 },
}

/** How well a layout protects against failing disks. */
export type Protection = 'none' | 'medium' | 'strong' | 'very_strong'

/**
 * What a pool on some disks would hold, in bytes, and how it would perform.
 * Estimates: ZFS keeps a little more for metadata and padding.
 */
export interface PoolEstimate {
  /** For data. */
  usable: number
  /** Taken by parity or mirror copies. */
  protection: number
  /** Beyond the smallest disk on larger ones, which ZFS leaves unused. */
  unused: number
  /** Sequential reads and writes as multiples of one disk; theoretical. */
  read: number
  write: number
  /** Disks that may fail without losing data. */
  tolerates: number
}

/** Why `count` disks do not suit `layout`, or null when they do. */
export function misfit(
  layout: NewPoolLayout,
  count: number,
): { minimum: number; maximum: number; reason: 'exactly' | 'tooFew' | 'tooMany' } | null {
  const { minimum, maximum } = LIMITS[layout]
  if (count >= minimum && count <= maximum) return null
  const reason = minimum === maximum ? 'exactly' : count < minimum ? 'tooFew' : 'tooMany'
  return { minimum, maximum, reason }
}

export function fits(layout: NewPoolLayout, count: number): boolean {
  return misfit(layout, count) === null
}

/** `current` while it fits `count` disks, else the first layout that does. */
export function fittingLayout(current: NewPoolLayout, count: number): NewPoolLayout {
  if (fits(current, count)) return current
  return POOL_LAYOUTS.find((layout) => fits(layout, count)) ?? current
}

/** The estimate for `layout` on disks of `sizes` bytes; null when they do not fit it. */
export function estimatePool(layout: NewPoolLayout, sizes: number[]): PoolEstimate | null {
  const n = sizes.length
  if (!fits(layout, n)) return null
  const smallest = Math.min(...sizes)
  const unused = sizes.reduce((sum, size) => sum + size, 0) - n * smallest
  if (layout === 'single') {
    return { usable: sizes[0]!, protection: 0, unused: 0, read: 1, write: 1, tolerates: 0 }
  }
  if (layout === 'mirror') {
    return {
      usable: smallest,
      protection: (n - 1) * smallest,
      unused,
      read: n,
      write: 1,
      tolerates: n - 1,
    }
  }
  // raidz1 to raidz3: as many disks' worth of parity.
  const p = Number(layout.slice('raidz'.length))
  return {
    usable: (n - p) * smallest,
    protection: p * smallest,
    unused,
    read: n - p,
    write: n - p,
    tolerates: p,
  }
}

export function protectionOf(tolerates: number): Protection {
  if (tolerates <= 0) return 'none'
  if (tolerates === 1) return 'medium'
  if (tolerates === 2) return 'strong'
  return 'very_strong'
}
