import { useQuery } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'

/** Addresses from DHCP can change; looking again now and then is enough. */
const REFRESH_MS = 30_000

export function useNetwork() {
  return useQuery({
    queryKey: ['network'],
    queryFn: () => unwrap(api.GET('/api/v1/network')),
    refetchInterval: REFRESH_MS,
  })
}
