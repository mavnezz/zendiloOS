import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type {
  BackupPlanRequest,
  ImageUpdateSettings,
  NewDockerProject,
  RestoreRequest,
} from '@/shared/api/types'

/** How often lists that change by themselves are asked again. */
const REFRESH = 10_000

export function useDockerOverview() {
  return useQuery({
    queryKey: ['docker', 'overview'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/overview')),
    refetchInterval: REFRESH,
  })
}

/** What the containers used over the last `seconds`. */
export function useUsage(seconds: Ref<number>, containers: Ref<string[]>) {
  return useQuery({
    queryKey: ['docker', 'usage', seconds, containers],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/docker/usage', {
          params: {
            query: {
              seconds: seconds.value,
              containers: containers.value.length ? containers.value.join(',') : undefined,
            },
          },
        }),
      ),
    refetchInterval: 20_000,
  })
}

export function useContainers() {
  return useQuery({
    queryKey: ['docker', 'containers'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/containers')),
    refetchInterval: REFRESH,
  })
}

export function useImages() {
  return useQuery({
    queryKey: ['docker', 'images'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/images')),
  })
}

export function useNetworks() {
  return useQuery({
    queryKey: ['docker', 'networks'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/networks')),
  })
}

export function useProjects() {
  return useQuery({
    queryKey: ['docker', 'projects'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/projects')),
    refetchInterval: REFRESH,
  })
}

export function useProject(name: Ref<string>) {
  return useQuery({
    queryKey: ['docker', 'projects', name],
    queryFn: () =>
      unwrap(api.GET('/api/v1/docker/projects/{name}', { params: { path: { name: name.value } } })),
    refetchInterval: REFRESH,
  })
}

export function useProjectCompose(name: Ref<string>, enabled: () => boolean) {
  return useQuery({
    queryKey: ['docker', 'projects', name, 'compose'],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/docker/projects/{name}/compose', {
          params: { path: { name: name.value } },
        }),
      ),
    enabled: computed(enabled),
  })
}

export function usePlaces() {
  return useQuery({
    queryKey: ['docker', 'places'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/places')),
  })
}

/** Lines a container or a project wrote; asked again while shown. */
export function useLogs(kind: 'containers' | 'projects', id: Ref<string>, enabled: () => boolean) {
  return useQuery({
    queryKey: ['docker', kind, id, 'logs'],
    queryFn: () =>
      kind === 'containers'
        ? unwrap(
            api.GET('/api/v1/docker/containers/{id}/logs', {
              params: { path: { id: id.value }, query: { lines: 500 } },
            }),
          )
        : unwrap(
            api.GET('/api/v1/docker/projects/{name}/logs', {
              params: { path: { name: id.value }, query: { lines: 500 } },
            }),
          ),
    enabled: computed(enabled),
    refetchInterval: 5000,
  })
}

/** What the Docker app did, newest first. */
export function useActivity() {
  return useQuery({
    queryKey: ['docker', 'activity'],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/logs/audit', { params: { query: { prefix: 'containers.', limit: 200 } } }),
      ),
  })
}

/** After any change: the engine's lists all may differ. */
function useRefresh() {
  const client = useQueryClient()
  return () => client.invalidateQueries({ queryKey: ['docker'] })
}

export type RunAction = 'start' | 'stop' | 'restart'

export function useContainerAction() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: ({ id, action }: { id: string; action: RunAction | 'remove' }) =>
      action === 'remove'
        ? unwrap(api.DELETE('/api/v1/docker/containers/{id}', { params: { path: { id } } }))
        : unwrap(
            api.POST('/api/v1/docker/containers/{id}/{action}', {
              params: { path: { id, action } },
            }),
          ),
    onSettled: refresh,
  })
}

export function useProjectAction() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: ({ name, action }: { name: string; action: RunAction | 'delete' }) => {
      const path = { params: { path: { name } } }
      if (action === 'delete') return unwrap(api.DELETE('/api/v1/docker/projects/{name}', path))
      return unwrap(
        api.POST('/api/v1/docker/projects/{name}/{action}', { params: { path: { name, action } } }),
      )
    },
    onSettled: refresh,
  })
}

export async function checkCompose(body: {
  name: string
  compose: string
  share_id?: number | null
  folder?: string | null
}) {
  return unwrap(api.POST('/api/v1/docker/compose/check', { body }))
}

export function useCreateProject() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: (body: NewDockerProject) => unwrap(api.POST('/api/v1/docker/projects', { body })),
    onSettled: refresh,
  })
}

export function useChangeCompose() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: ({
      name,
      compose,
      accept,
      pull = false,
    }: {
      name: string
      compose: string
      accept: boolean
      pull?: boolean
    }) =>
      unwrap(
        api.PUT('/api/v1/docker/projects/{name}/compose', {
          params: { path: { name } },
          body: { compose, accept_risks: accept, pull },
        }),
      ),
    onSettled: refresh,
  })
}

export function usePullImage() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: (reference: string) =>
      unwrap(api.POST('/api/v1/docker/images/pull', { body: { reference } })),
    onSettled: refresh,
  })
}

export function useRemoveImage() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.DELETE('/api/v1/docker/images/{id}', { params: { path: { id } } })),
    onSettled: refresh,
  })
}

export function usePruneImages() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: () => unwrap(api.POST('/api/v1/docker/images/prune')),
    onSettled: refresh,
  })
}

export function useCreateNetwork() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: (body: { name: string; subnet?: string | null; gateway?: string | null }) =>
      unwrap(api.POST('/api/v1/docker/networks', { body })),
    onSettled: refresh,
  })
}

export function useRemoveNetwork() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: (name: string) =>
      unwrap(api.DELETE('/api/v1/docker/networks/{name}', { params: { path: { name } } })),
    onSettled: refresh,
  })
}

export function useUpdateSettings() {
  return useQuery({
    queryKey: ['docker', 'updates', 'settings'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/updates/settings')),
  })
}

export function useSetUpdateSettings() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: ImageUpdateSettings) =>
      unwrap(api.PUT('/api/v1/docker/updates/settings', { body })),
    onSuccess: (settings) => client.setQueryData(['docker', 'updates', 'settings'], settings),
  })
}

/** The newest look for newer images; `null` before the first. */
export function useUpdates() {
  return useQuery({
    queryKey: ['docker', 'updates', 'report'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/updates')),
    refetchInterval: 60_000,
  })
}

export function useCheckUpdates() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: () => unwrap(api.POST('/api/v1/docker/updates/check')),
    onSuccess: (report) => client.setQueryData(['docker', 'updates', 'report'], report),
  })
}

export function useApplyUpdates() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: () => unwrap(api.POST('/api/v1/docker/updates/apply')),
    onSettled: refresh,
  })
}

export function useDataLocation() {
  return useQuery({
    queryKey: ['docker', 'data'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/data')),
  })
}

export function useMoveData() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: (pool: string) => unwrap(api.POST('/api/v1/docker/data/move', { body: { pool } })),
    onSettled: refresh,
  })
}

/** How often a project's log is read again while it is followed. */
export const FOLLOW_MS = 3000

/** The last `lines` lines of a project's containers; read again while `follow`. */
export function useProjectLog(name: Ref<string>, lines: Ref<number>, follow: Ref<boolean>) {
  return useQuery({
    queryKey: ['docker', 'projects', name, 'log', lines],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/docker/projects/{name}/logs', {
          params: { path: { name: name.value }, query: { lines: lines.value } },
        }),
      ),
    refetchInterval: () => (follow.value ? FOLLOW_MS : false),
    refetchIntervalInBackground: false,
  })
}

/**
 * Brings a project up again, with its images fetched anew if `pull`: the
 * job, whose output follows what Compose writes.
 */
export function useRedeploy() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: ({ name, pull }: { name: string; pull: boolean }) =>
      unwrap(
        api.POST('/api/v1/docker/projects/{name}/redeploy', {
          params: { path: { name } },
          body: { pull },
        }),
      ),
    onSettled: refresh,
  })
}

/** The containers whose image has a newer one, by name. */
export function useUpdatable() {
  const { data } = useUpdates()
  return computed(
    () =>
      new Set(
        (data.value?.images ?? [])
          .filter((image) => image.state === 'available')
          .flatMap((image) => image.containers),
      ),
  )
}

/** One container as Docker inspects it, asked again while shown. */
export function useContainerDetails(id: Ref<string>) {
  return useQuery({
    queryKey: ['docker', 'containers', id, 'details'],
    queryFn: () =>
      unwrap(api.GET('/api/v1/docker/containers/{id}', { params: { path: { id: id.value } } })),
    refetchInterval: REFRESH,
  })
}

/** The processes of a running container, asked again while shown. */
export function useContainerProcesses(id: Ref<string>, enabled: () => boolean) {
  return useQuery({
    queryKey: ['docker', 'containers', id, 'processes'],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/docker/containers/{id}/processes', { params: { path: { id: id.value } } }),
      ),
    enabled: computed(enabled),
    refetchInterval: 5000,
    retry: false,
  })
}

/** The last `lines` lines a container wrote; asked again every few seconds while `live`. */
export function useContainerLog(id: Ref<string>, lines: Ref<number>, live: Ref<boolean>) {
  return useQuery({
    queryKey: ['docker', 'containers', id, 'logs', lines],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/docker/containers/{id}/logs', {
          params: { path: { id: id.value }, query: { lines: lines.value } },
        }),
      ),
    refetchInterval: () => (live.value ? 3000 : false),
  })
}

/** How updating a container would go: by its project, or its image fetched only. */
export function containerUpdateWay(id: string) {
  return unwrap(api.GET('/api/v1/docker/containers/{id}/update', { params: { path: { id } } }))
}

export function useUpdateContainer() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.POST('/api/v1/docker/containers/{id}/update', { params: { path: { id } } })),
    onSettled: refresh,
  })
}

/** Where a terminal in a container is reached, on the page's own host. */
export function terminalUrl(
  id: string,
  command: string,
  columns: number,
  rows: number,
  page: Pick<Location, 'protocol' | 'host'> = window.location,
): string {
  const scheme = page.protocol === 'https:' ? 'wss' : 'ws'
  const query = new URLSearchParams({ command, columns: String(columns), rows: String(rows) })
  return `${scheme}://${page.host}/api/v1/docker/containers/${encodeURIComponent(id)}/terminal?${query.toString()}`
}

export function useVolumes() {
  return useQuery({
    queryKey: ['docker', 'volumes'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/volumes')),
    refetchInterval: REFRESH,
  })
}

/** What the volumes' data takes up: counted on the NAS, so asked apart and seldom. */
export function useVolumeSizes() {
  return useQuery({
    queryKey: ['docker-volume-sizes'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/volumes/sizes')),
    staleTime: 60_000,
  })
}

export function useRemoveVolume() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: (name: string) =>
      unwrap(api.DELETE('/api/v1/docker/volumes/{name}', { params: { path: { name } } })),
    onSettled: refresh,
  })
}

export function usePruneVolumes() {
  const refresh = useRefresh()
  return useMutation({
    mutationFn: () => unwrap(api.POST('/api/v1/docker/volumes/prune')),
    onSettled: refresh,
  })
}

/** Who may read share `id`, and with it every backup put there. */
export function useBackupReaders(id: Ref<number | null>) {
  return useQuery({
    queryKey: ['docker', 'backup-readers', id],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/docker/backup-shares/{id}/readers', {
          params: { path: { id: id.value ?? 0 } },
        }),
      ),
    enabled: computed(() => id.value !== null),
  })
}

/** Makes or replaces a volume's backup plan. */
export function saveBackupPlan(name: string, body: BackupPlanRequest) {
  return unwrap(
    api.PUT('/api/v1/docker/volumes/{name}/backup-plan', { params: { path: { name } }, body }),
  )
}

export function deleteBackupPlan(name: string) {
  return unwrap(
    api.DELETE('/api/v1/docker/volumes/{name}/backup-plan', { params: { path: { name } } }),
  )
}

/** A volume's backups, the newest first; asked again while one of them runs. */
export function useBackups(name: Ref<string>, running: Ref<boolean>) {
  return useQuery({
    queryKey: ['docker', 'volumes', name, 'backups'],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/docker/volumes/{name}/backups', {
          params: { path: { name: name.value } },
        }),
      ),
    refetchInterval: () => (running.value ? 3000 : false),
    enabled: computed(() => name.value !== ''),
  })
}

/** Backs a volume up now, as its plan says, in a job. */
export function backUpNow(name: string) {
  return unwrap(api.POST('/api/v1/docker/volumes/{name}/backups', { params: { path: { name } } }))
}

/** Restores backup `id`, in a job. */
export function restoreBackup(id: number, body: RestoreRequest) {
  return unwrap(api.POST('/api/v1/docker/backups/{id}/restore', { params: { path: { id } }, body }))
}

/** Asks the Docker app's lists again. */
export function useRefreshDocker() {
  return useRefresh()
}
