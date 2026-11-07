import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { AppProject, CatalogApp, InstalledApp } from '@/shared/api/types'
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
    permissions: { devices: [], capabilities: [], host_network: false, docker_socket: false },
    translations: {},
    digest: `sha256:${id}`,
    ...extra,
  }
}

const apps = ref<CatalogApp[]>([])
const installed = ref<Partial<InstalledApp>[]>([])
const projects = ref<Partial<AppProject>[]>([])

vi.mock('../api', async (original) => ({
  webUrl: (await original<typeof import('../api')>()).webUrl,
  useCatalog: () => ({ data: apps, error: ref(null), isPending: ref(false) }),
  useInstalled: () => ({ data: installed }),
  useAppProjects: () => ({ data: projects }),
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
  // Immich as a project that does not answer; Paperless installed the earlier way.
  projects.value = [
    { project: 'photos', source_id: 1, id: 'immich', run_state: 'stopped', web: null },
  ]
  installed.value = [
    {
      id: 'paperless',
      state: 'installed',
      run_state: 'running',
      web: { port: 8000, path: '/', https: false },
    },
  ]
  apps.value = [
    app('jellyfin', 'Jellyfin', {
      permissions: {
        devices: ['/dev/dri'],
        capabilities: [],
        host_network: false,
        docker_socket: false,
      },
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
    expect(rows[0]).toContain('Installed')
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
      .find((b) => b.text() === 'Installed')!
      .trigger('click')
    expect(wrapper.emitted('installed')).toHaveLength(1)
  })

  it('opens what a project made from an app answers on, and says what the earlier way left', () => {
    projects.value = [
      { project: 'photos', source_id: 1, id: 'immich', run_state: 'running', web: null },
      {
        project: 'photos-2',
        source_id: 1,
        id: 'immich',
        run_state: 'running',
        web: { port: 2284, path: '/', https: false },
      },
    ]
    installed.value = [
      { id: 'jellyfin', state: 'removing' },
      { id: 'paperless', state: 'failed' },
    ]
    const rows = store().findAll('li')

    expect(rows[0]!.find('a').attributes('href')).toContain(':2284/')
    expect(rows[1]!.text()).toContain('Being installed')
    expect(rows[2]!.text()).toContain('Failed')
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
        devices: ['/dev/dri', '/dev/dri/renderD128', '/dev/ttyUSB0', '/dev/video0', '/dev/mystery'],
        capabilities: ['NET_ADMIN', 'SYS_NICE'],
        host_network: true,
        docker_socket: true,
      },
    })
    expect(permissionKeys(asks)).toEqual([
      'gpu',
      'serial',
      'camera',
      'device',
      'netAdmin',
      'sysNice',
      'hostNetwork',
      'dockerSocket',
    ])
  })
})
