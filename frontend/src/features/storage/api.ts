import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { type MaybeRefOrGetter, toValue } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type {
  Job,
  NewPoolLayout,
  PoolPlan,
  ReplacePlan,
  SelfTestKind,
  SelfTestPlan,
  SelfTestPlans,
} from '@/shared/api/types'

/** Arrays change while they rebuild, so they are polled more often. */
const ARRAY_REFRESH_MS = 5_000
const DEVICE_REFRESH_MS = 30_000
/** SMART is read from the disks themselves; once in a while is enough. */
const SMART_STALE_MS = 5 * 60_000
const SELF_TEST_REFRESH_MS = 30_000

export function useStorageOverview() {
  return useQuery({
    queryKey: ['storage', 'overview'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/overview')),
    refetchInterval: DEVICE_REFRESH_MS,
  })
}

export function useDisks() {
  return useQuery({
    queryKey: ['storage', 'disks'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/disks')),
    refetchInterval: DEVICE_REFRESH_MS,
  })
}

export function useSmartReport(disk: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: ['storage', 'smart', disk],
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

export function useVolumes() {
  return useQuery({
    queryKey: ['storage', 'volumes'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/volumes')),
    refetchInterval: DEVICE_REFRESH_MS,
  })
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
