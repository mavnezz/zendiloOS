import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { PowerAction, SshRequest, TimeRequest } from '@/shared/api/types'

export function useSystemInfo() {
  return useQuery({
    queryKey: ['system', 'info'],
    queryFn: () => unwrap(api.GET('/api/v1/system/info')),
  })
}

export async function requestPowerAction(action: PowerAction): Promise<void> {
  await unwrap(api.POST('/api/v1/system/power', { body: { action } }))
}

/** Whether zendiloosd answers at all, e.g. while waiting for a restart. */
export async function isServerReachable(): Promise<boolean> {
  try {
    const response = await fetch('/api/v1/health', { cache: 'no-store' })
    return response.ok
  } catch {
    return false
  }
}

/** The NAS's time zone and how its clock is kept. */
export function useTime(enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: ['system', 'time'],
    queryFn: () => unwrap(api.GET('/api/v1/system/time')),
    enabled: computed(enabled),
  })
}

export function useSetTime() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: TimeRequest) => unwrap(api.PUT('/api/v1/system/time', { body })),
    onSuccess: (state) => client.setQueryData(['system', 'time'], state),
  })
}

/** The SSH server; looked at again while a timer is to turn it off. */
export function useSsh() {
  return useQuery({
    queryKey: ['system', 'ssh'],
    queryFn: () => unwrap(api.GET('/api/v1/system/ssh')),
    refetchInterval: (query) => (query.state.data?.off_at ? 30_000 : false),
  })
}

export function useSetSsh() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: SshRequest) => unwrap(api.PUT('/api/v1/system/ssh', { body })),
    onSuccess: (state) => client.setQueryData(['system', 'ssh'], state),
  })
}
