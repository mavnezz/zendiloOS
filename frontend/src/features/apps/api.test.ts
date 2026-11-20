import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

import { mountApp, settle } from '@/shared/test-support/mount'
import { called, on, reset } from '@/shared/test-support/server'

import {
  checkCompose,
  createProject,
  fillTemplate,
  listFolders,
  makeFolder,
  useCatalog,
  useInstallActions,
  useShareList,
  useSources,
} from './api'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

function hookHost(setup: () => void) {
  return defineComponent({
    setup() {
      setup()
      return () => h('div')
    },
  })
}

describe('the App Center’s requests', () => {
  it('picks up a recovered catalog without a click and stops polling after success', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    try {
      reset()
      const source = {
        id: 1,
        address: 'https://apps.example.org/index.json',
        official: true,
        added_at: 0,
        refreshed_at: null,
        refresh_error: 'Temporary failure in name resolution',
        skipped: 0,
      }
      on('GET', '/api/v1/apps/catalog', [])
      on('GET', '/api/v1/apps/sources', [source])
      let catalog: ReturnType<typeof useCatalog> | undefined
      let sources: ReturnType<typeof useSources> | undefined
      const Host = hookHost(() => {
        catalog = useCatalog()
        sources = useSources()
      })
      const { wrapper } = await mountApp(Host)
      expect(catalog!.data.value).toEqual([])
      expect(sources!.data.value?.[0]?.refresh_error).toContain('name resolution')

      // The background worker has now fetched the index successfully.
      on('GET', '/api/v1/apps/catalog', [{ id: 'jellyfin', name: 'Jellyfin' }])
      on('GET', '/api/v1/apps/sources', [{ ...source, refreshed_at: 1000, refresh_error: null }])
      await vi.advanceTimersByTimeAsync(30_000)
      await settle()
      expect(catalog!.data.value?.[0]?.id).toBe('jellyfin')
      expect(sources!.data.value?.[0]?.refresh_error).toBeNull()
      const catalogsRead = called('GET', '/api/v1/apps/catalog').length
      const sourcesRead = called('GET', '/api/v1/apps/sources').length
      await vi.advanceTimersByTimeAsync(60_000)
      await settle()
      expect(called('GET', '/api/v1/apps/catalog')).toHaveLength(catalogsRead)
      expect(called('GET', '/api/v1/apps/sources')).toHaveLength(sourcesRead)
      wrapper.unmount()
    } finally {
      vi.useRealTimers()
    }
  })

  it('list a share’s folders, sorted, and make one', async () => {
    reset()
    on('GET', '/api/v1/files/shares/3/folder', {
      writable: true,
      entries: [
        { name: 'Photos', kind: 'directory' },
        { name: 'notes.txt', kind: 'file' },
        { name: 'Albums', kind: 'directory' },
      ],
    })
    on('POST', '/api/v1/files/shares/3/folders', null)

    expect(await listFolders(3, 'Library')).toEqual({ names: ['Albums', 'Photos'], writable: true })
    expect(called('GET', '/api/v1/files/shares/3/folder')[0]?.query).toEqual({ path: 'Library' })
    await makeFolder(3, 'Library/New')
    expect(called('POST', '/api/v1/files/shares/3/folders')[0]?.body).toEqual({
      path: 'Library/New',
    })
  })

  it('offer the shares the administrator may use, by name, and stop apps', async () => {
    reset()
    on('GET', '/api/v1/files/shares', [
      { id: 2, name: 'photos', access: 'read', home: false },
      { id: 0, name: 'home', access: 'read_write', home: true },
      { id: 1, name: 'media', access: 'read_write', home: false },
    ])
    on('POST', '/api/v1/apps/installed/jellyfin/stop', null)
    let shares: ReturnType<typeof useShareList> | undefined
    let actions: ReturnType<typeof useInstallActions> | undefined
    const Host = hookHost(() => {
      shares = useShareList()
      actions = useInstallActions()
    })
    await mountApp(Host)

    expect(shares!.data.value).toEqual([
      { id: 1, name: 'media', writable: true },
      { id: 2, name: 'photos', writable: false },
    ])
    await actions!.stop('jellyfin')
    expect(called('POST', '/api/v1/apps/installed/jellyfin/stop')).toHaveLength(1)
  })

  it('fill in an app and make it a project named after it', async () => {
    reset()
    on('POST', '/api/v1/apps/catalog/1/jellyfin/compose', { compose: 'services: {}\n' })
    on('POST', '/api/v1/docker/compose/check', { risks: [], digest: 'c0ffee' })
    on('POST', '/api/v1/docker/projects', { job: { id: 7, kind: 'containers.project_deploy' } })
    const app = { source_id: 1, id: 'jellyfin' }

    const where = { name: 'media', share_id: 3, folder: 'docker/media' }
    const compose = await fillTemplate(app, { ...where, ports: { web: 8097 } })
    const place = { ...where, compose }
    expect(await checkCompose(place)).toEqual({ risks: [], digest: 'c0ffee' })
    const job = await createProject({
      ...place,
      accepted: null,
      template: { source_id: 1, app: 'jellyfin' },
    })

    expect(compose).toBe('services: {}\n')
    expect(called('POST', '/api/v1/apps/catalog/1/jellyfin/compose')[0]?.body).toEqual({
      ...where,
      ports: { web: 8097 },
    })
    expect(called('POST', '/api/v1/docker/projects')[0]?.body).toMatchObject({
      name: 'media',
      start: true,
      template: { source_id: 1, app: 'jellyfin' },
    })
    expect(job).toMatchObject({ id: 7 })
  })
})
