import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { UpdateSettings, UpdatesOverview } from '@/shared/api/types'

const KEY = ['system', 'updates']

/** Whether something is going on that the page follows closely. */
export function busy(overview: UpdatesOverview | undefined): boolean {
  return (
    overview?.phase === 'checking' ||
    overview?.phase === 'downloading' ||
    overview?.run?.state === 'running'
  )
}

/** What there is to update and what is going on; looked at often while something runs. */
export function useUpdates() {
  const query = useQuery({
    queryKey: KEY,
    queryFn: () => unwrap(api.GET('/api/v1/system/updates')),
    refetchInterval: (query) => (busy(query.state.data) ? 2000 : 30_000),
    // While zendiloOS installs itself, the server is away for a moment.
    retry: true,
    retryDelay: 2000,
  })
  return { ...query, busy: computed(() => busy(query.data.value)) }
}

export function useUpdateActions() {
  const client = useQueryClient()
  const refresh = () => client.invalidateQueries({ queryKey: KEY })
  return {
    check: async () => {
      await unwrap(api.POST('/api/v1/system/updates/check'))
      await refresh()
    },
    apply: async () => {
      await unwrap(api.POST('/api/v1/system/updates/apply'))
      await refresh()
    },
    save: async (settings: UpdateSettings) => {
      const saved = await unwrap(api.PUT('/api/v1/system/updates/settings', { body: settings }))
      await refresh()
      return saved
    },
  }
}
