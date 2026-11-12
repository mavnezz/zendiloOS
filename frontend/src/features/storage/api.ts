import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type MaybeRefOrGetter, toValue } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type {
  Enclosure,
  ImportPoolRequest,
  InventoryDisk,
  InventoryUpdate,
  Job,
  Locating,
  NewPoolLayout,
  PoolPlan,
  ReplacePlan,
  SelfTestKind,
  SelfTestPlan,
  SelfTestPlans,
  SetEnclosure,
} from '@/shared/api/types'

/** Arrays change while they rebuild, so they are polled more often. */
const ARRAY_REFRESH_MS = 5_000
const DEVICE_REFRESH_MS = 30_000
/** SMART is read from the disks themselves; once in a while is enough. */
const SMART_STALE_MS = 5 * 60_000
const SELF_TEST_REFRESH_MS = 30_000
/** While a disk blinks, its end follows the server. */
const LOCATE_REFRESH_MS = 5_000

/** The enclosure, which disk is in which slot, and the model the NAS was recognised as. */
export function useEnclosure() {
  return useQuery({
    queryKey: ['storage', 'enclosure'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/enclosure')),
    refetchInterval: DEVICE_REFRESH_MS,
  })
}

/** The NAS models zendiloOS knows; built in, so asked once. */
export function useEnclosureModels() {
  return useQuery({
    queryKey: ['storage', 'enclosure-models'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/enclosure/models')),
    staleTime: Number.POSITIVE_INFINITY,
  })
}

export function saveEnclosure(body: SetEnclosure): Promise<Enclosure> {
  return unwrap(api.PUT('/api/v1/storage/enclosure', { body }))
}

/** No enclosure: the disks are listed without slots. */
export function resetEnclosure(): Promise<Enclosure> {
  return unwrap(api.DELETE('/api/v1/storage/enclosure'))
}

/** The disks blinking now. */
export function useLocating() {
  return useQuery({
    queryKey: ['storage', 'locate'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/locate')),
    refetchInterval: (query) => (query.state.data?.length ? LOCATE_REFRESH_MS : false),
  })
}

/** Has the disk blink for `seconds` (60 if not given). */
export function startLocate(disk: string, seconds?: number): Promise<Locating> {
  return unwrap(
    api.POST('/api/v1/storage/disks/{name}/locate', {
      params: { path: { name: disk } },
      body: seconds == null ? {} : { seconds },
    }),
  )
}

export async function stopLocate(disk: string): Promise<void> {
  await unwrap(
    api.DELETE('/api/v1/storage/disks/{name}/locate', { params: { path: { name: disk } } }),
  )
}

/** Every disk seen, those present first. */
export function useInventory() {
  return useQuery({
    queryKey: ['storage', 'inventory'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/inventory')),
  })
}

export function saveInventory(serial: string, body: InventoryUpdate): Promise<InventoryDisk> {
  return unwrap(
    api.PUT('/api/v1/storage/inventory/{serial}', { params: { path: { serial } }, body }),
  )
}

/** Forgets a disk that is no longer present. */
export async function forgetDisk(serial: string): Promise<void> {
  await unwrap(api.DELETE('/api/v1/storage/inventory/{serial}', { params: { path: { serial } } }))
}

/** `enabled` false: not asked for now, e.g. while nobody needs to know. */
export function useStorageOverview(enabled: MaybeRefOrGetter<boolean> = true) {
  return useQuery({
    queryKey: ['storage', 'overview'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/overview')),
    refetchInterval: DEVICE_REFRESH_MS,
    enabled: computed(() => toValue(enabled)),
  })
}

export function useDisks() {
  return useQuery({
    queryKey: ['storage', 'disks'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/disks')),
    refetchInterval: DEVICE_REFRESH_MS,
  })
}

/** SMART of `disk`; asked only while `enabled`. */
export function useSmartReport(
  disk: MaybeRefOrGetter<string>,
  enabled: MaybeRefOrGetter<boolean> = true,
) {
  return useQuery({
    queryKey: ['storage', 'smart', disk],
    enabled: () => toValue(enabled),
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/storage/disks/{name}/smart', {
          params: { path: { name: toValue(disk) } },
        }),
      ),
    staleTime: SMART_STALE_MS,
    // While the disk tests itself, follow its progress.
    refetchInterval: (query) =>
      query.state.data?.running_self_test ? SELF_TEST_REFRESH_MS : false,
  })
}

/** Has the disk test itself; its SMART report then shows the progress. */
export async function startSelfTest(disk: string, kind: SelfTestKind): Promise<void> {
  await unwrap(
    api.POST('/api/v1/storage/disks/{name}/self-test', {
      params: { path: { name: disk } },
      body: { kind },
    }),
  )
}

export function useSelfTestPlans() {
  return useQuery({
    queryKey: ['storage', 'self-test-plans'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/self-test-plans')),
  })
}

/** Replaces every plan; a kind left out is not tested on a schedule. */
export function saveSelfTestPlans(plans: SelfTestPlan[]): Promise<SelfTestPlans> {
  return unwrap(api.PUT('/api/v1/storage/self-test-plans', { body: { plans } }))
}

/** Stops the disk's running self-test. */
export async function abortSelfTest(disk: string): Promise<void> {
  await unwrap(
    api.POST('/api/v1/storage/disks/{name}/self-test/abort', { params: { path: { name: disk } } }),
  )
}

export function useArrays() {
  return useQuery({
    queryKey: ['storage', 'arrays'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/arrays')),
    refetchInterval: ARRAY_REFRESH_MS,
  })
}

export function usePools() {
  return useQuery({
    queryKey: ['storage', 'pools'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/pools')),
    refetchInterval: ARRAY_REFRESH_MS,
  })
}

/**
 * The pools the configuration names that are not imported, found on the
 * disks or missing, and the others there: only for those who may import
 * them. Asked once and after storage jobs, not every few seconds:
 * `zpool import` reads every disk's labels, which wakes a disk asleep.
 */
export function useAbsentPools(enabled: MaybeRefOrGetter<boolean>) {
  return useQuery({
    queryKey: ['storage', 'pools', 'absent'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/pools/absent')),
    enabled: computed(() => toValue(enabled)),
  })
}

/** Imports a pool, or adopts one the configuration does not name: a job. */
export function importPool(body: ImportPoolRequest): Promise<Job> {
  return unwrap(api.POST('/api/v1/storage/pools/import', { body }))
}

/** Reads everything again, SMART included. */
export function useRefreshStorage() {
  const client = useQueryClient()
  return () => client.invalidateQueries({ queryKey: ['storage'] })
}

export function planPool(name: string, layout: NewPoolLayout, disks: string[]): Promise<PoolPlan> {
  return unwrap(api.POST('/api/v1/storage/pools/plan', { body: { name, layout, disks } }))
}

export function createPool(token: string, confirm: string): Promise<Job> {
  return unwrap(api.POST('/api/v1/storage/pools', { body: { token, confirm } }))
}

export function destroyPool(name: string, confirm: string): Promise<Job> {
  return unwrap(
    api.POST('/api/v1/storage/pools/{name}/destroy', {
      params: { path: { name } },
      body: { confirm },
    }),
  )
}

export function planReplace(pool: string, device: string, disk: string): Promise<ReplacePlan> {
  return unwrap(
    api.POST('/api/v1/storage/pools/{name}/replace/plan', {
      params: { path: { name: pool } },
      body: { device, disk },
    }),
  )
}

export function replaceDisk(pool: string, token: string): Promise<Job> {
  return unwrap(
    api.POST('/api/v1/storage/pools/{name}/replace', {
      params: { path: { name: pool } },
      body: { token },
    }),
  )
}

export async function scrubPool(pool: string): Promise<void> {
  await unwrap(api.POST('/api/v1/storage/pools/{name}/scrub', { params: { path: { name: pool } } }))
}

/** Stops a running scrub; what it checked so far stays checked. */
export async function stopScrub(pool: string): Promise<void> {
  await unwrap(
    api.POST('/api/v1/storage/pools/{name}/scrub/stop', { params: { path: { name: pool } } }),
  )
}
