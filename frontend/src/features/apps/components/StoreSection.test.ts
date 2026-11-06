import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { CatalogApp, InstalledApp } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import { appText, permissionKeys } from '../texts'
import StoreSection from './StoreSection.vue'

function app(id: string, name: string, extra: Partial<CatalogApp> = {}): CatalogApp {
  return {
    source_id: 1,
    official: true,
    id,
    name,
    version: '1.0',
    tagline: `${name} on the NAS`,
    categories: ['media'],
    permissions: { devices: [], capabilities: [], host_network: false },
    translations: {},
    digest: `sha256:${id}`,
    ...extra,
  }
}

const apps = ref<CatalogApp[]>([])
const installed = ref<Partial<InstalledApp>[]>([])
const update = vi.fn(() => Promise.resolve())

vi.mock('../api', async (original) => ({
  webUrl: (await original<typeof import('../api')>()).webUrl,
  useCatalog: () => ({ data: apps, error: ref(null), isPending: ref(false) }),
  useInstalled: () => ({ data: installed }),
  useInstallActions: () => ({ update }),
  iconUrl: (a: CatalogApp) => `/icon/${a.id}`,
  useDocker: () => ({
    data: ref({ installed: true, running: true, volume: { pool: 'tank', number: 1 }, volumes: [] }),
  }),
  useEnableDocker: () => () => Promise.resolve(),
}))

function store(props: { category?: string | null; search?: string } = {}) {
  return mount(StoreSection, {
    props: { category: null, search: '', ...props },
    global: { plugins: [i18n] },
  })
}

beforeEach(() => {
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  update.mockClear()
  installed.value = [
    {
      id: 'immich',
      name: 'Immich',
      translations: {},
      state: 'installed',
      run_state: 'running',
      update: '2.1',
      web: null,
    },
    {
      id: 'paperless',
      state: 'installed',
      run_state: 'running',
      update: null,
      web: { port: 8000, path: '/', https: false },
    },
  ]
  apps.value = [
    app('jellyfin', 'Jellyfin', {
      permissions: { devices: ['/dev/dri'], capabilities: [], host_network: false },
    }),
    app('immich', 'Immich', {
      categories: ['photos', 'backup'],
      translations: {
        de: { tagline: 'Fotos vom Handy sichern', name: null, description: null, notes: null },
      },
    }),
    app('paperless', 'Paperless', { official: false, categories: ['productivity'] }),
  ]
})

describe('the app store', () => {
  it('lists every app by name with its kind and what can be done', async () => {
    const wrapper = store()

    const rows = wrapper.findAll('li').map((li) => li.text())
    expect(rows).toHaveLength(3)
    expect(rows[0]).toContain('Immich')
    expect(rows[0]).toContain('Photos · Backup')
    expect(rows[0]).toContain('Update')
    expect(rows[1]).toContain('Jellyfin')
    expect(rows[1]).toContain('Install')
    expect(wrapper.findAll('li')[2]!.find('a').attributes('href')).toContain(':8000/')

    await wrapper
      .findAll('li')[1]!
      .findAll('button')
      .find((b) => b.text() === 'Install')!
      .trigger('click')
    expect(wrapper.emitted('install')?.[0]?.[0]).toMatchObject({ id: 'jellyfin' })
    await wrapper
      .findAll('li')[0]!
      .findAll('button')
      .find((b) => b.text() === 'Update')!
      .trigger('click')
    expect(update).toHaveBeenCalledWith('immich', false)
  })

  it('shows one category, or what the search finds', () => {
    expect(store({ search: 'paper' }).findAll('li')).toHaveLength(1)
    const photos = store({ category: 'photos' })
    expect(photos.findAll('li').map((li) => li.text())).toEqual([expect.stringContaining('Immich')])
    expect(photos.text()).toContain('Photos')
  })
})

describe('app texts', () => {
  it('use the interface language and fall back to English', () => {
    const immich = apps.value[1]!
    expect(appText(immich, 'tagline', 'de')).toBe('Fotos vom Handy sichern')
    expect(appText(immich, 'name', 'de')).toBe('Immich')
    expect(appText(immich, 'tagline', 'fr')).toBe('Immich on the NAS')
  })

  it('name each kind of permission once', () => {
    const asks = app('x', 'X', {
      permissions: {
        devices: ['/dev/dri', '/dev/dri/renderD128'],
        capabilities: ['NET_ADMIN'],
        host_network: true,
      },
    })
    expect(permissionKeys(asks)).toEqual(['gpu', 'device', 'netAdmin', 'hostNetwork'])
  })
})
