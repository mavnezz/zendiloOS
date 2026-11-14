import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type { UpsRequest } from '@/shared/api/types'

/** How often the UPS's state is looked at while its page is open. */
const LOOK_EVERY_MS = 10_000
/** And while it matters more: on battery, or before its driver has answered. */
const LOOK_SOON_MS = 3_000

/**
 * The UPS: whether NUT is installed, the settings, and what the UPS says of
 * itself. Looked at again while one is set, as a power cut does not wait
 * for a reload; sooner while it does not answer, which it does not for a
 * few seconds after a change.
 */
export function useUps() {
  return useQuery({
    queryKey: ['ups'],
    queryFn: () => unwrap(api.GET('/api/v1/ups')),
    refetchInterval: (query) => {
      const overview = query.state.data
      if (!overview?.settings.source) return false
      return overview.reading && !overview.reading.on_battery ? LOOK_EVERY_MS : LOOK_SOON_MS
    },
  })
}

export function useSetUps() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: UpsRequest) => unwrap(api.PUT('/api/v1/ups', { body })),
    onSuccess: (overview) => client.setQueryData(['ups'], overview),
  })
}

/** Installs Network UPS Tools, as a job. */
export function installUps() {
  return unwrap(api.POST('/api/v1/ups/install'))
}

/** The UPSs NUT finds on USB now. */
export async function scanUps() {
  return (await unwrap(api.POST('/api/v1/ups/scan'))).devices
}
