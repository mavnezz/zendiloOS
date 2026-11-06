import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { CatalogApp } from '@/shared/api/types'

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
