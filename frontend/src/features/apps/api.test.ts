import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

import { mountApp } from '@/desktop/testing/mount'
import { called, on, reset } from '@/desktop/testing/server'

import { listFolders, makeFolder, useInstallActions, useShareList } from './api'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/desktop/testing/server')).routed(await original()),
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
    expect(called('POST', '/api/v1/files/shares/3/folders')[0]?.body).toEqual({ path: 'Library/New' })
  })

  it('offer the shares by name and install, retry and stop apps', async () => {
    reset()
    on('GET', '/api/v1/shares', [
      { id: 2, name: 'photos' },
      { id: 1, name: 'media' },
    ])
    on('POST', '/api/v1/apps/installed', { job: { id: 1 } })
    on('POST', '/api/v1/apps/installed/jellyfin/retry', { id: 2 })
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
      { id: 1, name: 'media' },
      { id: 2, name: 'photos' },
    ])
    await actions!.install({
      source: 1,
      app: 'jellyfin',
      ports: {},
      shares: {},
      settings: {},
      confirm_permissions: false,
      pool: null,
    })
    await actions!.retry('jellyfin')
    expect(called('POST', '/api/v1/apps/installed')[0]?.body).toMatchObject({ app: 'jellyfin' })
    expect(called('POST', '/api/v1/apps/installed/jellyfin/retry')).toHaveLength(1)
  })
})
