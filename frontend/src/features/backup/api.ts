import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type {
  BackupRestoreRequest,
  BackupServer,
  BackupSettings,
  BackupTaskRequest,
} from '@/shared/api/types'

/** rsync installed, and the NAS's public key for SSH servers once made. */
export function useBackupPlugin() {
  return useQuery({
    queryKey: ['backup', 'plugin'],
    queryFn: () => unwrap(api.GET('/api/v1/backup/plugin')),
  })
}

/** The tasks: asked again every few seconds while one runs, every half minute otherwise. */
export function useBackupTasks(enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: ['backup', 'tasks'],
    queryFn: () => unwrap(api.GET('/api/v1/backup/tasks')),
    enabled: computed(enabled),
    refetchInterval: (query) =>
      (query.state.data ?? []).some((task) => task.running) ? 3000 : 30_000,
  })
}

/** The runs of one task, or of all. */
export function useBackupRuns(task: () => number | null, enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: computed(() => ['backup', 'runs', task()]),
    queryFn: () => {
      const id = task()
      return id == null
        ? unwrap(api.GET('/api/v1/backup/runs', { params: { query: { limit: 200 } } }))
        : unwrap(
            api.GET('/api/v1/backup/tasks/{id}/runs', {
              params: { path: { id }, query: { limit: 200 } },
            }),
          )
    },
    enabled: computed(enabled),
    refetchInterval: 15_000,
  })
}

export function useBackupSettings() {
  return useQuery({
    queryKey: ['backup', 'settings'],
    queryFn: () => unwrap(api.GET('/api/v1/backup/settings')),
  })
}

/** Who reads what is backed up into share `id`. */
export function useShareReaders(id: () => number | null) {
  return useQuery({
    queryKey: computed(() => ['backup', 'readers', id()]),
    queryFn: () =>
      unwrap(api.GET('/api/v1/backup/shares/{id}/readers', { params: { path: { id: id()! } } })),
    enabled: computed(() => id() != null),
  })
}

/** The removable drives plugged in, mounted where they can be. */
export function useDrives() {
  return useQuery({
    queryKey: ['storage', 'removable'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/removable')),
  })
}

export function installBackup() {
  return unwrap(api.POST('/api/v1/backup/plugin/install'))
}

export function publicKey() {
  return unwrap(api.POST('/api/v1/backup/key'))
}

export function hostKeys(host: string, port: number) {
  return unwrap(api.POST('/api/v1/backup/servers/host-keys', { body: { host, port } }))
}

export function checkServer(
  server: BackupServer,
  path: string,
  password: string | null,
  task: number | null,
) {
  return unwrap(
    api.POST('/api/v1/backup/servers/check', { body: { server, path, password, task_id: task } }),
  )
}

export function versions(id: number) {
  return unwrap(api.GET('/api/v1/backup/tasks/{id}/versions', { params: { path: { id } } }))
}

export function browse(id: number, version: number | null, path: string) {
  return unwrap(
    api.GET('/api/v1/backup/tasks/{id}/browse', {
      params: { path: { id }, query: { version: version ?? undefined, path } },
    }),
  )
}

export function transfers(run: number, offset = 0) {
  return unwrap(
    api.GET('/api/v1/backup/runs/{id}/transfers', {
      params: { path: { id: run }, query: { offset, limit: 1000 } },
    }),
  )
}

/** What changes tasks, each refreshing what it changed. */
export function useBackupActions() {
  const client = useQueryClient()
  const refresh = () => client.invalidateQueries({ queryKey: ['backup'] })
  const after = async <T>(done: Promise<T>): Promise<T> => {
    const value = await done
    void refresh()
    return value
  }
  return {
    create: (body: BackupTaskRequest) => after(unwrap(api.POST('/api/v1/backup/tasks', { body }))),
    update: (id: number, body: BackupTaskRequest) =>
      after(unwrap(api.PUT('/api/v1/backup/tasks/{id}', { params: { path: { id } }, body }))),
    remove: (id: number) =>
      after(unwrap(api.DELETE('/api/v1/backup/tasks/{id}', { params: { path: { id } } }))),
    run: (id: number) =>
      after(unwrap(api.POST('/api/v1/backup/tasks/{id}/run', { params: { path: { id } } }))),
    stop: (id: number) =>
      after(unwrap(api.POST('/api/v1/backup/tasks/{id}/stop', { params: { path: { id } } }))),
    restore: (id: number, body: BackupRestoreRequest) =>
      after(
        unwrap(api.POST('/api/v1/backup/tasks/{id}/restore', { params: { path: { id } }, body })),
      ),
    settings: (body: BackupSettings) => after(unwrap(api.PUT('/api/v1/backup/settings', { body }))),
  }
}
