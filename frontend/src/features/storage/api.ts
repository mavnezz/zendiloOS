import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { type MaybeRefOrGetter, toValue } from 'vue'

import { api, unwrap } from '@/shared/api/client'

/** Arrays change while they rebuild, so they are polled more often. */
const ARRAY_REFRESH_MS = 5_000
const DEVICE_REFRESH_MS = 30_000
/** SMART is read from the disks themselves; once in a while is enough. */
const SMART_STALE_MS = 5 * 60_000

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
  })
}

export function useArrays() {
  return useQuery({
    queryKey: ['storage', 'arrays'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/arrays')),
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
