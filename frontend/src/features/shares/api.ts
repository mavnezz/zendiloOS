import { useQuery, useQueryClient } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type { CreateShareRequest, Job, Share, UpdateShareRequest } from '@/shared/api/types'

export function useShares() {
  return useQuery({
    queryKey: ['shares'],
    queryFn: () => unwrap(api.GET('/api/v1/shares')),
  })
}

/** What rules can name: accounts and groups (needs `users.manage`). */
export function useSubjects() {
  const accounts = useQuery({
    queryKey: ['accounts'],
    queryFn: () => unwrap(api.GET('/api/v1/accounts')),
  })
  const groups = useQuery({
    queryKey: ['groups'],
    queryFn: () => unwrap(api.GET('/api/v1/groups')),
  })
  return { accounts, groups }
}

/** Volumes a share can live on: pools zendiloOS created and mounted. */
export function useVolumes() {
  return useQuery({
    queryKey: ['storage', 'pools'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/pools')),
    select: (pools) => pools.filter((pool) => pool.volume != null && pool.mount_point != null),
  })
}

export function useShareActions() {
  const client = useQueryClient()
  const refreshing = async <T>(request: Promise<T>): Promise<T> => {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['shares'] })
    }
  }
  const path = (id: number) => ({ params: { path: { id } } })
  return {
    create: (body: CreateShareRequest): Promise<Share> =>
      refreshing(unwrap(api.POST('/api/v1/shares', { body }))),
    update: (id: number, body: UpdateShareRequest): Promise<{ share: Share; job?: Job | null }> =>
      refreshing(unwrap(api.PATCH('/api/v1/shares/{id}', { ...path(id), body }))),
    remove: (id: number, confirm: string): Promise<Job> =>
      refreshing(
        unwrap(api.POST('/api/v1/shares/{id}/delete', { ...path(id), body: { confirm } })),
      ),
  }
}
