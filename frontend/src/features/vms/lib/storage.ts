// The locations of the VMs' storage as their page shows them: how full each
// pool is, what the VMs take of it, which pools could hold one more, and
// whether one may go.

import type { VmLocation, VmOnLocation, VmPlugin, VmPoolInfo } from '@/shared/api/types'

/** How full the location's pool is, 0 to 1; none while it cannot be read. */
export function poolFill(location: VmLocation): number | null {
  const used = location.pool_used_bytes
  const free = location.available_bytes
  if (used == null || free == null || used + free <= 0) return null
  return used / (used + free)
}

/** What a VM's disks take of the location's whole, 0 to 1; none while not known. */
export function vmShare(vm: VmOnLocation, location: VmLocation): number | null {
  if (vm.used_bytes == null || !location.used_bytes) return null
  return Math.min(1, vm.used_bytes / location.used_bytes)
}

/** The pools zendiloOS made that hold no location yet, by volume. */
export function freePools(plugin: VmPlugin | undefined): VmPoolInfo[] {
  const taken = new Set((plugin?.locations ?? []).map((location) => location.pool))
  return [...(plugin?.pools ?? [])]
    .filter((pool) => !taken.has(pool.pool))
    .sort((a, b) => a.volume - b.volume)
}

/**
 * Why a location cannot go, if it cannot: VMs' disks on it, images of the
 * library, or it is the library's while there are others. The server
 * checks the same.
 */
export function removalBlocked(
  location: VmLocation,
  locations: VmLocation[],
): 'vms' | 'images' | 'library' | null {
  if (location.vms.length) return 'vms'
  if (location.images > 0) return 'images'
  if (location.library && locations.length > 1) return 'library'
  return null
}
