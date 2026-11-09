import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import { type MaybeRefOrGetter, toValue } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type {
  CreateShareRequest,
  FileServices,
  Job,
  Share,
  ShareSnapshot,
  ShareSnapshotSchedule,
  UpdateShareRequest,
} from '@/shared/api/types'

/** Every share (needs `shares.manage`); `enabled` false asks for none. */
export function useShares(options: { enabled?: MaybeRefOrGetter<boolean> } = {}) {
  return useQuery({
    queryKey: ['shares'],
    queryFn: () => unwrap(api.GET('/api/v1/shares')),
    enabled: () => toValue(options.enabled ?? true),
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
      // The file manager's tree shows the folders on the volumes too.
      await client.invalidateQueries({ queryKey: ['files', 'volumes'] })
      await client.invalidateQueries({ queryKey: ['files', 'shares'] })
    }
  }
  const path = (id: number) => ({ params: { path: { id } } })
  return {
    create: (body: CreateShareRequest): Promise<Share> =>
      refreshing(unwrap(api.POST('/api/v1/shares', { body }))),
    update: (id: number, body: UpdateShareRequest): Promise<Share> =>
      refreshing(unwrap(api.PATCH('/api/v1/shares/{id}', { ...path(id), body }))),
    remove: (id: number, confirm: string): Promise<Job> =>
      refreshing(
        unwrap(api.POST('/api/v1/shares/{id}/delete', { ...path(id), body: { confirm } })),
      ),
  }
}

/** A share's snapshots, newest first, and its schedules. */
export function useAdminSnapshots(id: MaybeRefOrGetter<number | null>) {
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
    /** Returns the share to the snapshot; answers the later snapshots that went. */
    rollback: (id: number, label: string, confirm: string): Promise<string[]> =>
      refreshing(
        id,
        unwrap(
          api.POST('/api/v1/shares/{id}/snapshots/{label}/rollback', {
            params: { path: { id, label } },
            body: { confirm },
          }),
        ),
      ).then((answer) => answer.destroyed),
    schedule: (id: number, body: ShareSnapshotSchedule[]): Promise<ShareSnapshotSchedule[]> =>
      refreshing(
        id,
        unwrap(
          api.PUT('/api/v1/shares/{id}/snapshots/schedules', { params: { path: { id } }, body }),
        ),
      ),
  }
}

/** Which file services run, and Samba's settings for all shares. */
export function useFileServices() {
  return useQuery({
    queryKey: ['file-services'],
    queryFn: () => unwrap(api.GET('/api/v1/file-services')),
  })
}

export function useSetFileServices() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: FileServices) => unwrap(api.PUT('/api/v1/file-services', { body })),
    onSuccess: (stored) => client.setQueryData(['file-services'], stored),
  })
}
