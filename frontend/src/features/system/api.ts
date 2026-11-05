import { useQuery } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'

export function useSystemInfo() {
  return useQuery({
    queryKey: ['system', 'info'],
    queryFn: () => unwrap(api.GET('/api/v1/system/info')),
  })
}
