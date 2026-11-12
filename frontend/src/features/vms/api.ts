import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, onScopeDispose, shallowRef, type Ref } from 'vue'

import { ApiProblem, api, csrfToken, unwrap } from '@/shared/api/client'
import { live } from '@/shared/api/live'
import type {
  CreateVmRequest,
  Job,
  RestoredVm,
  UpdateVmRequest,
  VirtualMachine,
  VmBackupTarget,
  VmImage,
  VmRestoreAs,
  VmSnapshotSchedule,
  VmStats,
} from '@/shared/api/types'

import { settling } from './lib/vms'

/** What the machine offers and where the VMs' storage is. */
export function useVmPlugin() {
  return useQuery({
    queryKey: ['vms', 'plugin'],
    queryFn: () => unwrap(api.GET('/api/v1/vms/plugin')),
  })
}

/**
 * The VMs with what libvirt says they do: asked again every few seconds
 * while one is being made, starts or shuts down, and every half minute
 * otherwise (a guest may power itself off).
 */
export function useVms(enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: ['vms', 'list'],
    queryFn: () => unwrap(api.GET('/api/v1/vms')),
    enabled: computed(enabled),
    refetchInterval: (query) => ((query.state.data ?? []).some(settling) ? 3000 : 30_000),
  })
}

/** The image library: asked again every few seconds while one is being added. */
export function useVmImages(enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: ['vms', 'images'],
    queryFn: () => unwrap(api.GET('/api/v1/vms/images')),
    enabled: computed(enabled),
    refetchInterval: (query) =>
      (query.state.data ?? []).some((image) => !image.ready) ? 3000 : false,
  })
}

/** The locations of the VMs' storage with what lies on them, the library's first. */
export function useVmLocations(enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: ['vms', 'locations'],
    queryFn: () => unwrap(api.GET('/api/v1/vms/locations')),
    enabled: computed(enabled),
    refetchInterval: 30_000,
  })
}

/** Locations added and removed; each reads the plugin and the locations again. */
export function useLocationActions() {
  const client = useQueryClient()
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['vms'] })
    }
  }
  return {
    add: (pool: string) =>
      refreshing(unwrap(api.POST('/api/v1/vms/locations', { body: { pool } }))),
    remove: (pool: string, confirm: string) =>
      refreshing(
        unwrap(
          api.POST('/api/v1/vms/locations/{pool}/delete', {
            params: { path: { pool } },
            body: { confirm },
          }),
        ),
      ),
  }
}

/**
 * Internal networks, made and removed through the network's own API: a
 * bridge without members, kept at once (it carries no address, so nothing
 * the browser reaches the NAS by changes; the confirmation proves that
 * anyway, as the Network page's does). Each reads the plugin again.
 */
export function useInternalNetworks() {
  const client = useQueryClient()
  async function keep<T>(request: Promise<T>): Promise<T> {
    try {
      const done = await request
      await unwrap(api.POST('/api/v1/network/confirm'))
      return done
    } finally {
      await client.invalidateQueries({ queryKey: ['vms'] })
      await client.invalidateQueries({ queryKey: ['network'] })
    }
  }
  return {
    add: (name: string) =>
      keep(
        unwrap(
          api.POST('/api/v1/network/devices', { body: { kind: 'bridge', name, members: [] } }),
        ),
      ),
    remove: (name: string) =>
      keep(unwrap(api.DELETE('/api/v1/network/devices/{name}', { params: { path: { name } } }))),
  }
}

/** Changes to VMs; each reads the list again. */
export function useVmActions() {
  const client = useQueryClient()
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['vms'] })
    }
  }
  const id = (id: string) => ({ params: { path: { id } } })
  return {
    create: (body: CreateVmRequest) => refreshing(unwrap(api.POST('/api/v1/vms', { body }))),
    update: (vm: string, body: UpdateVmRequest): Promise<VirtualMachine> =>
      refreshing(unwrap(api.PUT('/api/v1/vms/{id}', { ...id(vm), body }))),
    start: (vm: string): Promise<VirtualMachine> =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/start', id(vm)))),
    shutDown: (vm: string): Promise<VirtualMachine> =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/shutdown', id(vm)))),
    forceOff: (vm: string): Promise<VirtualMachine> =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/force-off', id(vm)))),
    restart: (vm: string): Promise<VirtualMachine> =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/restart', id(vm)))),
    setAutostart: (vm: string, autostart: boolean) =>
      refreshing(unwrap(api.PUT('/api/v1/vms/{id}/autostart', { ...id(vm), body: { autostart } }))),
    remove: (vm: string, confirm: string) =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/delete', { ...id(vm), body: { confirm } }))),
  }
}

/** How often a running upload tells the server the person is still there. */
const KEEP_ALIVE_MS = 5 * 60 * 1000

function uploadProblem(request: XMLHttpRequest): ApiProblem {
  try {
    const body = JSON.parse(request.responseText) as { code?: string; detail?: string }
    return new ApiProblem(request.status, body.code ?? 'unknown', body.detail)
  } catch {
    return new ApiProblem(request.status, 'unknown')
  }
}

/**
 * Sends `file` into the library as `name`. Through XMLHttpRequest, because
 * fetch cannot report how much of a request body went out.
 */
export function uploadImage(
  file: Blob,
  name: string,
  options: { onProgress: (fraction: number) => void; signal?: AbortSignal },
): Promise<VmImage> {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest()
    request.open('POST', `/api/v1/vms/images/upload?${new URLSearchParams({ name })}`)
    const token = csrfToken()
    if (token) request.setRequestHeader('X-CSRF-Token', token)
    request.setRequestHeader('Content-Type', 'application/octet-stream')
    request.upload.onprogress = (event) => {
      if (event.lengthComputable && event.total > 0) options.onProgress(event.loaded / event.total)
    }
    // A long upload is the person's doing: it keeps the session alive.
    const alive = setInterval(() => {
      void fetch('/api/v1/auth/session', { credentials: 'same-origin' }).catch(() => undefined)
    }, KEEP_ALIVE_MS)
    const done = (settle: () => void) => () => {
      clearInterval(alive)
      settle()
    }
    request.onload = done(() =>
      request.status < 300
        ? resolve(JSON.parse(request.responseText) as VmImage)
        : reject(uploadProblem(request)),
    )
    request.onerror = done(() => reject(new ApiProblem(0, 'network')))
    request.onabort = done(() => reject(new ApiProblem(0, 'aborted')))
    options.signal?.addEventListener('abort', () => request.abort())
    request.send(file)
  })
}

/** Changes to the library; each reads it again. */
export function useImageActions() {
  const client = useQueryClient()
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['vms', 'images'] })
    }
  }
  const id = (id: string) => ({ params: { path: { id } } })
  return {
    importFromShare: (shareId: number, path: string, name: string) =>
      refreshing(
        unwrap(api.POST('/api/v1/vms/images', { body: { share_id: shareId, path, name } })),
      ),
    upload: (
      file: Blob,
      name: string,
      onProgress: (fraction: number) => void,
      signal?: AbortSignal,
    ) => refreshing(uploadImage(file, name, { onProgress, signal })),
    rename: (image: string, name: string) =>
      refreshing(unwrap(api.PUT('/api/v1/vms/images/{id}', { ...id(image), body: { name } }))),
    remove: (image: string) => refreshing(unwrap(api.DELETE('/api/v1/vms/images/{id}', id(image)))),
  }
}

/**
 * What the VMs use, every few seconds while it is looked at: the live
 * topic subscribed for as long as the calling component lives.
 */
export function useVmStats(): Ref<VmStats | null> {
  const stats = shallowRef<VmStats | null>(null)
  const stop = live.subscribe('vm_stats', (data) => (stats.value = data))
  onScopeDispose(stop)
  return stats
}

/** The console's WebSocket on this page's own origin. */
export function consoleUrl(
  vm: string,
  page: Pick<Location, 'protocol' | 'host'> = window.location,
): string {
  const scheme = page.protocol === 'https:' ? 'wss' : 'ws'
  return `${scheme}://${page.host}/api/v1/vms/${encodeURIComponent(vm)}/console`
}

/** The host's USB and PCI devices and the IOMMU, for a VM to be given. */
export function useVmDevices(enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: ['vms', 'devices'],
    queryFn: () => unwrap(api.GET('/api/v1/vms/devices')),
    enabled: computed(enabled),
    refetchInterval: 15_000,
  })
}

/** A VM's snapshots, newest first, and its schedules. */
export function useVmSnapshots(vm: () => string | null) {
  return useQuery({
    queryKey: computed(() => ['vms', 'snapshots', vm()]),
    queryFn: () =>
      unwrap(api.GET('/api/v1/vms/{id}/snapshots', { params: { path: { id: vm()! } } })),
    enabled: computed(() => vm() !== null),
  })
}

/** Snapshots taken, rolled back and deleted, schedules set; each reads them again. */
export function useSnapshotActions() {
  const client = useQueryClient()
  async function refreshing<T>(vm: string, request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['vms', 'snapshots', vm] })
    }
  }
  const id = (vm: string) => ({ id: vm })
  return {
    take: (vm: string, name: string) =>
      refreshing(
        vm,
        unwrap(
          api.POST('/api/v1/vms/{id}/snapshots', { params: { path: id(vm) }, body: { name } }),
        ),
      ),
    rollback: (vm: string, label: string, newer: boolean) =>
      refreshing(
        vm,
        unwrap(
          api.POST('/api/v1/vms/{id}/snapshots/{label}/rollback', {
            params: { path: { id: vm, label } },
            body: { newer },
          }),
        ),
      ),
    remove: (vm: string, label: string) =>
      refreshing(
        vm,
        unwrap(
          api.POST('/api/v1/vms/{id}/snapshots/{label}/delete', {
            params: { path: { id: vm, label } },
          }),
        ),
      ),
    setSchedules: (vm: string, schedules: VmSnapshotSchedule[]) =>
      refreshing(
        vm,
        unwrap(
          api.PUT('/api/v1/vms/{id}/snapshots/schedules', {
            params: { path: id(vm) },
            body: { schedules },
          }),
        ),
      ),
  }
}

/** Every VM's backups, newest first: asked again often while `busy` says one is made. */
export function useVmBackups(
  enabled: () => boolean = () => true,
  busy: () => boolean = () => false,
) {
  return useQuery({
    queryKey: ['vms', 'backups'],
    queryFn: () => unwrap(api.GET('/api/v1/vms/backups')),
    enabled: computed(enabled),
    refetchInterval: () => (busy() ? 3000 : 30_000),
  })
}

/** The removable drives plugged in, as storage has them: where a backup can go. */
export function useVmDrives(enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: ['storage', 'removable'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/removable')),
    enabled: computed(enabled),
    refetchInterval: 10_000,
  })
}

/** Backups made, canceled, restored and deleted; each reads the VMs and backups again. */
export function useBackupActions() {
  const client = useQueryClient()
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['vms'] })
    }
  }
  return {
    backUp: (vm: string, target: VmBackupTarget, folder: string): Promise<Job> =>
      refreshing(
        unwrap(
          api.POST('/api/v1/vms/{id}/backups', {
            params: { path: { id: vm } },
            body: { target, folder },
          }),
        ),
      ),
    cancel: (vm: string) =>
      refreshing(
        unwrap(api.POST('/api/v1/vms/{id}/backups/cancel', { params: { path: { id: vm } } })),
      ),
    restore: (backup: number, as: VmRestoreAs): Promise<RestoredVm> =>
      refreshing(
        unwrap(
          api.POST('/api/v1/vms/backups/{backup}/restore', {
            params: { path: { backup } },
            body: { as },
          }),
        ),
      ),
    remove: (backup: number) =>
      refreshing(
        unwrap(api.POST('/api/v1/vms/backups/{backup}/delete', { params: { path: { backup } } })),
      ),
  }
}
