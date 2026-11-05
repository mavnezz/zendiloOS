import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { type MaybeRefOrGetter, toValue } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type {
  CreateShareRequest,
  Job,
  Share,
  ShareSnapshot,
  ShareSnapshotSchedule,
  UpdateShareRequest,
} from '@/shared/api/types'

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

/** A share's snapshots, newest first, and its schedules. */
export function useShareSnapshots(id: MaybeRefOrGetter<number | null>) {
  return useQuery({
    queryKey: ['shares', id, 'snapshots'],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/shares/{id}/snapshots', { params: { path: { id: toValue(id) ?? 0 } } }),
      ),
    enabled: () => toValue(id) != null,
  })
}

export function useSnapshotActions() {
  const client = useQueryClient()
  const refreshing = async <T>(id: number, request: Promise<T>): Promise<T> => {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['shares', id, 'snapshots'] })
    }
  }
  return {
    take: (id: number, note: string): Promise<ShareSnapshot> =>
      refreshing(
        id,
        unwrap(
          api.POST('/api/v1/shares/{id}/snapshots', { params: { path: { id } }, body: { note } }),
        ),
      ),
    remove: (id: number, label: string): Promise<void> =>
      refreshing(
        id,
        unwrap(
          api.POST('/api/v1/shares/{id}/snapshots/{label}/delete', {
            params: { path: { id, label } },
          }),
        ),
      ),
    schedule: (id: number, body: ShareSnapshotSchedule[]): Promise<ShareSnapshotSchedule[]> =>
      refreshing(
        id,
        unwrap(
          api.PUT('/api/v1/shares/{id}/snapshots/schedules', { params: { path: { id } }, body }),
        ),
      ),
  }
}
