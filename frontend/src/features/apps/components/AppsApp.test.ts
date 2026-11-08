import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import { click, mountApp, page, press, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { components } from '@/shared/api/schema'
import type { AppSource, CatalogApp, CatalogAppDetail, InstalledApp } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { WINDOW_SECTION } from '@/shared/lib/window-section'

import AppsApp from './AppsApp.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)
const none = { devices: [], capabilities: [], host_network: false, docker_socket: false }

function app(id: string, extra: Partial<CatalogApp> = {}): CatalogApp {
  return {
    source_id: 1,
    official: true,
    id,
    name: id[0]!.toUpperCase() + id.slice(1),
    version: '1.0',
    tagline: `${id} on the NAS`,
    categories: ['media'],
    permissions: none,
    translations: {},
    digest: `sha256:${id}`,
    ...extra,
  }
}

function detail(id: string, extra: Partial<CatalogAppDetail> = {}): CatalogAppDetail {
  return {
    ...app(id),
    architectures: ['amd64'],
    compose: 'services:\n  app:\n    image: x\n',
    description: `All about ${id}.`,
    developer: 'Someone',
    license: 'MIT',
    notes: 'First paragraph.\n\nSecond paragraph.',
    ports: [{ id: 'web', label: 'Web', default: 8096, fixed: false }],
    settings: [
      {
        id: 'admin',
        label: 'Admin name',
        kind: 'text',
        generate: false,
        required: true,
        default: 'admin',
      },
      {
        id: 'secret',
        label: 'Secret',
        kind: 'password',
        generate: true,
        required: true,
        length: 32,
      },
    ],
    shares: [{ id: 'media', label: 'Media', description: 'Films and music', read_write: false }],
    source_code: 'https://example.org/src',
    web: { port: 'web', path: '/', https: false },
    website: 'https://example.org',
    ...extra,
  }
}

function installed(id: string, extra: Partial<InstalledApp> = {}): InstalledApp {
  return {
    id,
    source_id: 1,
    name: id[0]!.toUpperCase() + id.slice(1),
    version: '1.0',
    tagline: `${id} on the NAS`,
    translations: {},
    digest: `sha256:${id}`,
    state: 'installed',
    run_state: 'running',
    web: { port: 8096, path: '/', https: false },
    ports: { web: 8096 },
    shares: {},
    permissions: none,
    pool: 'tank',
    installed_at: 0,
    updated_at: 0,
    ...extra,
  }
}

const ready: components['schemas']['DockerSetup'] = {
  installed: true,
  running: true,
  version: '27.5',
  volume: { pool: 'tank', number: 1 },
  volumes: [
    { pool: 'tank', number: 1 },
    { pool: 'cold', number: 2 },
  ],
}

const official: AppSource = {
  id: 1,
  address: 'https://catalog.zendiloos.org',
  official: true,
  added_at: 0,
  refreshed_at: 1_790_000_000,
  skipped: 0,
}

const section = ref('all')

async function open(name: string): Promise<VueWrapper> {
  section.value = name
  const { wrapper } = await mountApp(AppsApp, {
    global: {
      provide: {
        [WINDOW_SECTION as symbol]: { section, navigate: (next: string) => (section.value = next) },
      },
    },
  })
  return wrapper
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/apps/catalog', [
    app('jellyfin'),
    app('immich', { categories: ['photos'], official: false, source_id: 2 }),
  ])
  on('GET', '/api/v1/apps/installed', [])
  on('GET', '/api/v1/apps/projects', [])
  on('GET', '/api/v1/docker/places', [{ share_id: 1, share: 'docker' }])
  on('GET', '/api/v1/files/shares', [{ id: 1, name: 'docker', access: 'read_write', home: false }])
  on('GET', '/api/v1/apps/docker', ready)
  on('GET', '/api/v1/apps/sources', [
    official,
    {
      id: 2,
      address: 'https://apps.example.org',
      official: false,
      added_at: 0,
      refreshed_at: null,
      skipped: 2,
      refresh_error: 'unreachable',
    },
  ])
})

describe('the App Center', () => {
  it('shows the store by category and searches it', async () => {
    const wrapper = await open('all')
    expect(page()).toContain('Jellyfin')
    expect(page()).toContain('Immich')
    expect(page()).toContain(t('appStore.categories.photos'))

    section.value = 'category:photos'
    await settle()
    expect(page()).not.toContain('Jellyfin')

    section.value = 'sources'
    await settle()
    await type(`input[aria-label="${t('appStore.search')}"]`, 'jelly')
    expect(section.value).toBe('all')
    expect(wrapper.findComponent({ name: 'StoreSection' }).exists()).toBe(true)
  })

  it('shows an app in full and installs it', async () => {
    on(
      'GET',
      '/api/v1/apps/catalog/2/immich',
      detail('immich', {
        official: false,
        source_id: 2,
        permissions: { ...none, devices: ['/dev/dri'] },
      }),
    )
    const wrapper = await open('all')
    wrapper.findComponent({ name: 'StoreSection' }).vm.$emit('open', { source_id: 2, id: 'immich' })
    await settle()

    expect(page()).toContain('All about immich.')
    expect(page()).toContain('Second paragraph.')
    expect(page()).toContain('apps.example.org')
    expect(page()).toContain(t('appStore.unofficial'))
    expect(page()).toContain('Films and music')
    expect(page()).toContain('/dev/dri')
    await click(new RegExp(t('appStore.compose')))
    expect(page()).toContain('image: x')
    await click(new RegExp(`^\\s*${t('appStore.install')}\\s*$`))
    expect(wrapper.findComponent({ name: 'InstallDialog' }).props('open')).toBe(true)
    await press(t('appStore.back'))
    expect(wrapper.findComponent({ name: 'AppDetail' }).exists()).toBe(false)
  })

  it('opens an app made into a project, and makes another from it', async () => {
    on('GET', '/api/v1/apps/projects', [
      {
        project: 'media',
        source_id: 1,
        id: 'jellyfin',
        name: 'Jellyfin',
        tagline: '',
        translations: {},
        digest: 'sha256:jellyfin',
        run_state: 'running',
        web: { port: 9000, path: '/', https: false },
        created_at: 0,
      },
    ])
    on('GET', '/api/v1/apps/catalog/1/jellyfin', detail('jellyfin'))
    const wrapper = await open('all')
    expect(page()).toContain(t('appStore.installed.open'))
    wrapper
      .findComponent({ name: 'StoreSection' })
      .vm.$emit('open', { source_id: 1, id: 'jellyfin' })
    await settle()

    const opener = [...document.querySelectorAll('a')].find(
      (a) => a.textContent?.trim() === t('appStore.installed.open'),
    )!
    expect(opener.getAttribute('href')).toBe(`http://${window.location.hostname}:9000/`)
    await click(t('appStore.installAgain'))
    expect(wrapper.findComponent({ name: 'InstallDialog' }).props('open')).toBe(true)
    expect(page()).toContain(t('appStore.placeTitle'))
  })

  it('leads from an installed app without a web interface to the installed ones', async () => {
    on('GET', '/api/v1/apps/installed', [installed('jellyfin', { web: null })])
    on('GET', '/api/v1/apps/catalog/1/jellyfin', detail('jellyfin', { web: null }))
    const wrapper = await open('all')
    wrapper
      .findComponent({ name: 'StoreSection' })
      .vm.$emit('open', { source_id: 1, id: 'jellyfin' })
    await settle()
    await click(t('appStore.isInstalled'))
    expect(section.value).toBe('installed')
  })

  it('installs Docker on a pool first', async () => {
    on('GET', '/api/v1/apps/docker', { ...ready, installed: false, running: false, volume: null })
    on('POST', '/api/v1/apps/docker/install', {
      id: 4,
      kind: 'apps.docker_install',
      state: 'running',
      step: 'packages',
      target: null,
      created_at: 0,
      updated_at: 0,
    })
    await open('all')
    expect(section.value).toBe('all')
    section.value = 'docker'
    await settle()
    expect(page()).toContain(t('appStore.docker.notInstalled'))

    await click(t('appStore.install'))
    await type('#docker-pool', 'cold')
    await submit('#docker-place')
    expect(called('POST', '/api/v1/apps/docker/install')[0]?.body).toEqual({ pool: 'cold' })
    expect(page()).toContain(t('appStore.docker.steps.packages'))
  })

  it('puts Docker’s data on a pool once it is installed', async () => {
    on('GET', '/api/v1/apps/docker', { ...ready, volume: null })
    on('POST', '/api/v1/apps/docker', problem(409, 'refused', 'Docker is missing'))
    await open('docker')
    await click(t('appStore.docker.choosePlace'))
    await submit('#docker-place')
    expect(called('POST', '/api/v1/apps/docker')[0]?.body).toEqual({ pool: 'tank' })
    expect(page()).toContain(t('errors.refused'))
  })

  it('lists Docker among the plugins, opens it and leads back', async () => {
    await open('plugins')
    expect(page()).toContain(t('appStore.plugins.title'))
    expect(page()).toContain('Docker')
    expect(page()).toContain(t('appStore.docker.running'))
    const card = [...document.querySelectorAll('button')].find((b) =>
      b.textContent?.includes(t('appStore.docker.subtitle')),
    )!
    card.click()
    await settle()
    expect(section.value).toBe('docker')
    expect(page()).toContain(t('appStore.docker.toStore'))
    // Back by the button above the page, and by the navigation.
    const titled = (inNav: boolean) =>
      [...document.querySelectorAll('button')].find(
        (b) =>
          Boolean(b.closest('nav')) === inNav &&
          b.textContent?.trim() === t('appStore.plugins.title'),
      )!
    titled(false).click()
    await settle()
    expect(section.value).toBe('plugins')
    section.value = 'docker'
    await settle()
    titled(true).click()
    await settle()
    expect(section.value).toBe('plugins')

    // The ACME client is the second plugin, opened and left the same way.
    expect(page()).toContain(t('appStore.acme.name'))
    ;[...document.querySelectorAll('button')]
      .find((b) => b.textContent?.includes(t('appStore.acme.subtitle')))!
      .click()
    await settle()
    expect(section.value).toBe('acme')
    titled(false).click()
    await settle()
    expect(section.value).toBe('plugins')
  })

  it('sends a ready engine to the store', async () => {
    await open('docker')
    expect(page()).toContain(t('appStore.docker.running'))
    await click(t('appStore.docker.toStore'))
    expect(section.value).toBe('all')
  })

  it('adds, refreshes and removes catalogs', async () => {
    on('POST', '/api/v1/apps/sources', {
      ...official,
      id: 3,
      address: 'https://more.example.org',
      official: false,
    })
    on('POST', '/api/v1/apps/sources/refresh', [])
    on('DELETE', '/api/v1/apps/sources/2', problem(409, 'official_source'))
    await open('sources')
    expect(page()).toContain('unreachable')

    await type('#source-address', ' https://more.example.org ')
    ;(document.querySelector('form input[type="checkbox"]') as HTMLInputElement).click()
    await settle()
    await submit('form')
    expect(called('POST', '/api/v1/apps/sources')[0]?.body).toEqual({
      address: 'https://more.example.org',
    })
    await click(new RegExp(t('appStore.sources.refresh')))
    expect(called('POST', '/api/v1/apps/sources/refresh')).toHaveLength(1)
    await press(t('appStore.sources.remove'))
    expect(called('DELETE', '/api/v1/apps/sources/2')).toHaveLength(1)
    expect(page()).toContain(t('errors.official_source'))
  })

  it('runs, reads and removes installed apps', async () => {
    on('GET', '/api/v1/apps/installed', [
      installed('jellyfin'),
      installed('immich', { run_state: 'stopped' }),
    ])
    on('POST', '/api/v1/apps/installed/jellyfin/stop', null)
    on('POST', '/api/v1/apps/installed/immich/start', problem(502, 'host_failed', 'no engine'))
    on('GET', '/api/v1/apps/installed/jellyfin/logs', { text: 'jellyfin-1  | started\n' })
    on('DELETE', '/api/v1/apps/installed/jellyfin', {
      id: 5,
      kind: 'apps.remove',
      state: 'running',
    })
    const wrapper = await open('installed')
    const menus = () => wrapper.findAllComponents({ name: 'ActionMenu' })

    menus()[0]!.vm.$emit('select', 'start')
    await settle()
    expect(called('POST', '/api/v1/apps/installed/immich/start')).toHaveLength(1)
    menus()[1]!.vm.$emit('select', 'stop')
    await settle()
    expect(called('POST', '/api/v1/apps/installed/jellyfin/stop')).toHaveLength(1)
    menus()[1]!.vm.$emit('select', 'logs')
    await settle()
    expect(page()).toContain('started')
    menus()[1]!.vm.$emit('select', 'remove')
    await settle()
    await type('#remove-confirm', 'jellyfin')
    await submit('[role="dialog"] form')
    expect(called('DELETE', '/api/v1/apps/installed/jellyfin')[0]?.body).toEqual({
      confirm: 'jellyfin',
    })
  })

  it('names the step of a removal and says what went wrong', async () => {
    on('GET', '/api/v1/apps/installed', [
      installed('jellyfin'),
      installed('immich', { state: 'failed', run_state: 'stopped' }),
      installed('kuma', { state: 'removing', run_state: null }),
    ])
    on('GET', '/api/v1/apps/installed/jellyfin/logs', problem(502, 'host_failed', 'gone'))
    on('DELETE', '/api/v1/apps/installed/jellyfin', problem(422, 'confirmation_mismatch'))
    const wrapper = await open('installed')
    useJobsStore().update({
      id: 1,
      kind: 'apps.uninstall',
      target: 'kuma',
      state: 'running',
      step: 'account',
      created_at: 0,
      updated_at: 0,
      output: [],
    })
    await settle()
    expect(page()).toContain(t('appStore.installed.steps.account'))
    expect(page()).toContain(t('appStore.installed.states.failed'))

    const row = wrapper
      .findAll('li')
      .find(
        (li) => li.text().includes('Jellyfin') && li.findComponent({ name: 'ActionMenu' }).exists(),
      )!
    const menu = row.findComponent({ name: 'ActionMenu' })
    menu.vm.$emit('select', 'logs')
    await settle()
    expect(page()).toContain('gone')
    await click(t('appStore.installed.reload'))
    expect(called('GET', '/api/v1/apps/installed/jellyfin/logs')).toHaveLength(2)
    menu.vm.$emit('select', 'remove')
    await settle()
    await type('#remove-confirm', 'wrong')
    await submit('[role="dialog"] form')
    expect(page()).toContain(t('errors.confirmation_mismatch'))
  })

  it('leads from an empty list to the store', async () => {
    await open('installed')
    expect(page()).toContain(t('appStore.installed.empty'))
    await click(t('appStore.installed.toStore'))
    expect(section.value).toBe('all')
  })
})
