import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

import { mountApp } from '@/shared/test-support/mount'
import { called, on, reset } from '@/shared/test-support/server'

import {
  checkCompose,
  createProject,
  fillTemplate,
  listFolders,
  makeFolder,
  useInstallActions,
  useShareList,
} from './api'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

describe('the App Center’s requests', () => {
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
    const Host = defineComponent({
      setup() {
        shares = useShareList()
        actions = useInstallActions()
        return () => h('div')
      },
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
