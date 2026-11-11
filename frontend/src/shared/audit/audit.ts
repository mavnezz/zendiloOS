// The audit log as several pages show it: the control panel's whole log,
// an app's own entries (`prefix`, such as `vms.`), searched on the server,
// a page at a time, and all of it for an export.

import { useInfiniteQuery } from '@tanstack/vue-query'
import { computed, type MaybeRefOrGetter, toValue } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { AuditRecord } from '@/shared/api/types'

/** Entries a page shows. */
export const AUDIT_PAGE = 50
/** The most the server gives at once, and the most an export takes in all. */
const EXPORT_PAGE = 500
export const EXPORT_MAX = 20_000

export interface AuditFilter {
  /** Only actions that start so, such as `vms.`. */
  prefix?: string
  /** What the actor, action, target, address or reason hold. */
  search?: string
}

function query(filter: AuditFilter, limit: number, before?: number) {
  const search = filter.search?.trim()
  return {
    limit,
    before,
    prefix: filter.prefix || undefined,
    search: search ? search : undefined,
  }
}

/** The audit log, newest first, a page at a time; more pages load older entries. */
export function useAuditPages(filter: MaybeRefOrGetter<AuditFilter>, size = AUDIT_PAGE) {
  return useInfiniteQuery({
    queryKey: computed(() => [
      'logs',
      'audit',
      toValue(filter).prefix ?? '',
      toValue(filter).search ?? '',
      size,
    ]),
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET('/api/v1/logs/audit', {
          params: { query: query(toValue(filter), size, pageParam ?? undefined) },
        }),
      ),
    initialPageParam: null as number | null,
    getNextPageParam: (page) => page.older ?? undefined,
  })
}

/** Every entry the filter takes, newest first, at most [`EXPORT_MAX`]. */
export async function allAuditRecords(filter: AuditFilter): Promise<AuditRecord[]> {
  const records: AuditRecord[] = []
  let before: number | undefined
  while (records.length < EXPORT_MAX) {
    const page = await unwrap(
      api.GET('/api/v1/logs/audit', { params: { query: query(filter, EXPORT_PAGE, before) } }),
    )
    records.push(...page.entries)
    if (page.older == null) break
    before = page.older
  }
  return records.slice(0, EXPORT_MAX)
}
