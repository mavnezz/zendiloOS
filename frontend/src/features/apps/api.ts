import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { CatalogApp, FillTemplateRequest } from '@/shared/api/types'

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

/** The apps installed the earlier way, with what Docker makes of them. */
export function useInstalled() {
  return useQuery({
    queryKey: ['apps', 'installed'],
    queryFn: () => unwrap(api.GET('/api/v1/apps/installed')),
  })
}

/** The Docker projects made from apps of a catalog, with where they answer. */
export function useAppProjects() {
  return useQuery({
    queryKey: ['apps', 'projects'],
    queryFn: () => unwrap(api.GET('/api/v1/apps/projects')),
  })
}

/**
 * The app's Compose file filled in with `values`, for the signed-in
 * administrator and the project named in them; refused if it breaks its
 * catalog's policy as Compose reads it there.
 */
export async function fillTemplate(
  app: { source_id: number; id: string },
  values: FillTemplateRequest,
): Promise<string> {
  const filled = await unwrap(
    api.POST('/api/v1/apps/catalog/{source}/{app}/compose', {
      params: { path: { source: app.source_id, app: app.id } },
      body: values,
    }),
  )
  return filled.compose
}

/** The shares a project's folder can go in: those the administrator may write. */
export function usePlaces() {
  return useQuery({
    queryKey: ['docker', 'places'],
    queryFn: () => unwrap(api.GET('/api/v1/docker/places')),
  })
}

/**
 * What a Compose file in `folder` of a share would let its containers do,
 * and the digest of what was checked, which accepting it names.
 */
export async function checkCompose(body: {
  name: string
  compose: string
  share_id: number
  folder: string
}) {
  return unwrap(api.POST('/api/v1/docker/compose/check', { body }))
}

/** Makes the filled-in file a Docker project and deploys it; the job that does. */
export async function createProject(body: {
  name: string
  share_id: number
  folder: string
  compose: string
  /** The check's digest, when its risks were accepted. */
  accepted: string | null
  template: { source_id: number; app: string }
}) {
  const made = await unwrap(api.POST('/api/v1/docker/projects', { body: { ...body, start: true } }))
  return made.job ?? null
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

/**
 * Running and removing the apps installed the earlier way; each reads the
 * list again. `refresh` reads the projects made from apps again too.
 */
export function useInstallActions() {
  const client = useQueryClient()
  const refresh = () =>
    Promise.all([
      client.invalidateQueries({ queryKey: ['apps', 'installed'] }),
      client.invalidateQueries({ queryKey: ['apps', 'projects'] }),
    ])
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await refresh()
    }
  }
  const path = (id: string) => ({ params: { path: { id } } })
  return {
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

/**
 * The shares an app can be given: those the signed-in administrator may
 * use, as the file manager has them; the app runs as them.
 */
export { listFolders, makeFolder, useShareList } from '@/shared/ui/folder-picker/folders'
