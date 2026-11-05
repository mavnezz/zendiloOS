import { useQuery } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type { PowerAction } from '@/shared/api/types'

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
