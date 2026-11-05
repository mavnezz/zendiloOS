import { useInfiniteQuery } from '@tanstack/vue-query'
import { type MaybeRefOrGetter, toValue } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { LogPriority, LogSource } from '@/shared/api/types'

const PAGE = 200

/** The journal, newest first; more pages load older entries. */
export function useJournal(
  source: MaybeRefOrGetter<LogSource>,
  priority: MaybeRefOrGetter<LogPriority>,
) {
  return useInfiniteQuery({
    queryKey: ['logs', 'journal', source, priority],
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET('/api/v1/logs/journal', {
          params: {
            query: {
              source: toValue(source),
              priority: toValue(priority),
              limit: PAGE,
              before: pageParam ?? undefined,
            },
          },
        }),
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.older ?? undefined,
  })
}

/** The audit log, newest first; more pages load older records. */
export function useAuditLog() {
  return useInfiniteQuery({
    queryKey: ['logs', 'audit'],
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET('/api/v1/logs/audit', {
          params: { query: { limit: PAGE, before: pageParam ?? undefined } },
        }),
      ),
    initialPageParam: null as number | null,
    getNextPageParam: (page) => page.older ?? undefined,
  })
}
