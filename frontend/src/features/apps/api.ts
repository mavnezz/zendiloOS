import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { CatalogApp, InstallAppRequest } from '@/shared/api/types'

export function useCatalog() {
  return useQuery({
    queryKey: ['apps', 'catalog'],
    queryFn: () => unwrap(api.GET('/api/v1/apps/catalog')),
  })
}

/** One app's details; `app` null asks for none. */
export function useCatalogApp(app: Ref<{ source: number; id: string } | null>) {
  return useQuery({
    queryKey: ['apps', 'catalog', app],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/apps/catalog/{source}/{app}', {
          params: { path: { source: app.value?.source ?? 0, app: app.value?.id ?? '' } },
        }),
      ),
    enabled: computed(() => app.value !== null),
  })
}

export function useSources() {
  return useQuery({
    queryKey: ['apps', 'sources'],
    queryFn: () => unwrap(api.GET('/api/v1/apps/sources')),
  })
}

/** Where the browser loads an app's icon; a new version is a new address. */
export function iconUrl(app: Pick<CatalogApp, 'source_id' | 'id' | 'digest'>): string {
  const version = encodeURIComponent(app.digest)
  return `/api/v1/apps/catalog/${app.source_id}/${encodeURIComponent(app.id)}/icon?v=${version}`
}

/** Changes to the sources; each reads the sources and the catalog again. */
export function useSourceActions() {
  const client = useQueryClient()
  const refresh = () => client.invalidateQueries({ queryKey: ['apps'] })
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await refresh()
    }
  }
  return {
    add: (address: string) =>
      refreshing(unwrap(api.POST('/api/v1/apps/sources', { body: { address } }))),
    remove: (id: number) =>
      refreshing(unwrap(api.DELETE('/api/v1/apps/sources/{id}', { params: { path: { id } } }))),
    refreshAll: () => refreshing(unwrap(api.POST('/api/v1/apps/sources/refresh'))),
  }
}

/** Whether apps can run: Docker installed, its data on a pool. */
export function useDocker() {
  return useQuery({
    queryKey: ['apps', 'docker'],
    queryFn: () => unwrap(api.GET('/api/v1/apps/docker')),
  })
}

/** Puts Docker's data on `pool`; Docker restarts. */
export function useEnableDocker() {
  const client = useQueryClient()
  return async (pool: string) => {
    const setup = await unwrap(api.POST('/api/v1/apps/docker', { body: { pool } }))
    client.setQueryData(['apps', 'docker'], setup)
    return setup
  }
}

/** Installs Docker and puts its data on `pool`, as a job. */
export async function installDocker(pool: string) {
  return unwrap(api.POST('/api/v1/apps/docker/install', { body: { pool } }))
}

/** The installed apps, with what Docker makes of them. */
export function useInstalled() {
  return useQuery({
    queryKey: ['apps', 'installed'],
    queryFn: () => unwrap(api.GET('/api/v1/apps/installed')),
  })
}

/** Where the app's web interface is, on the address the NAS was reached at. */
export function webUrl(
  app: { web?: { port: number; path: string; https: boolean } | null },
  host = window.location.hostname,
): string | null {
  if (!app.web) return null
  const scheme = app.web.https ? 'https' : 'http'
  const bracketed = host.includes(':') ? `[${host}]` : host
  return `${scheme}://${bracketed}:${app.web.port}${app.web.path}`
}

/** Installing, running and removing apps; each reads the list again. */
export function useInstallActions() {
  const client = useQueryClient()
  const refresh = () => client.invalidateQueries({ queryKey: ['apps', 'installed'] })
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await refresh()
    }
  }
  const path = (id: string) => ({ params: { path: { id } } })
  return {
    install: (request: InstallAppRequest) =>
      refreshing(unwrap(api.POST('/api/v1/apps/installed', { body: request }))),
    retry: (id: string) =>
      refreshing(unwrap(api.POST('/api/v1/apps/installed/{id}/retry', path(id)))),
    /** `confirmed`: the digest whose permissions were confirmed, if any. */
    update: (id: string, confirmed: string | null) =>
      refreshing(
        unwrap(
          api.POST('/api/v1/apps/installed/{id}/update', {
            ...path(id),
            body: { confirm_permissions: confirmed !== null, digest: confirmed },
          }),
        ),
      ),
    start: (id: string) =>
      refreshing(unwrap(api.POST('/api/v1/apps/installed/{id}/start', path(id)))),
    stop: (id: string) =>
      refreshing(unwrap(api.POST('/api/v1/apps/installed/{id}/stop', path(id)))),
    uninstall: (id: string, confirm: string) =>
      refreshing(
        unwrap(api.DELETE('/api/v1/apps/installed/{id}', { ...path(id), body: { confirm } })),
      ),
    logs: (id: string, lines = 500) =>
      unwrap(
        api.GET('/api/v1/apps/installed/{id}/logs', { params: { path: { id }, query: { lines } } }),
      ),
    refresh,
  }
}

/** The shares an app can be given (the same list the Shares window shows). */
export function useShareList() {
  return useQuery({
    queryKey: ['shares'],
    queryFn: () => unwrap(api.GET('/api/v1/shares')),
    select: (shares) =>
      shares
        .map((share) => ({ id: share.id, name: share.name }))
        .sort((a, b) => a.name.localeCompare(b.name)),
  })
}

/** The folders in one folder of a share, as the signed-in administrator sees them. */
export async function listFolders(
  share: number,
  path: string,
): Promise<{ names: string[]; writable: boolean }> {
  const folder = await unwrap(
    api.GET('/api/v1/files/shares/{id}/folder', {
      params: { path: { id: share }, query: { path } },
    }),
  )
  const names = folder.entries
    .filter((entry) => entry.kind === 'directory')
    .map((entry) => entry.name)
    .sort((a, b) => a.localeCompare(b))
  return { names, writable: folder.writable }
}

/** Makes the folder `path` in a share. */
export async function makeFolder(share: number, path: string): Promise<void> {
  await unwrap(
    api.POST('/api/v1/files/shares/{id}/folders', {
      params: { path: { id: share } },
      body: { path },
    }),
  )
}
