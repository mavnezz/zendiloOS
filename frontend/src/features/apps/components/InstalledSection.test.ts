import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { computed, ref } from 'vue'

import type { AppProject, InstalledApp } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { WINDOW_SECTION } from '@/shared/lib/window-section'

import { webUrl } from '../api'
import InstalledSection from './InstalledSection.vue'

const start = vi.fn<(id: string) => Promise<void>>(() => Promise.resolve())
const refresh = vi.fn(() => Promise.resolve())
const earlier = ref<InstalledApp[]>([])
const projects = ref<AppProject[]>([])

vi.mock('../api', async (original) => ({
  ...(await original<typeof import('../api')>()),
  useInstalled: () => ({ data: earlier, error: ref(null), isPending: ref(false) }),
  useAppProjects: () => ({ data: projects, error: ref(null), isPending: ref(false) }),
  useInstallActions: () => ({ start: (id: string) => start(id), refresh }),
}))

function app(id: string, extra: Partial<InstalledApp> = {}): InstalledApp {
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
    web: { port: 8096, path: '/web', https: false },
    ports: { web: 8096 },
    shares: {},
    permissions: { devices: [], capabilities: [], host_network: false, docker_socket: false },
    pool: 'tank',
    installed_at: 0,
    updated_at: 0,
    ...extra,
  }
}

function project(name: string, id: string, extra: Partial<AppProject> = {}): AppProject {
  return {
    project: name,
    source_id: 1,
    id,
    name: id[0]!.toUpperCase() + id.slice(1),
    tagline: '',
    translations: {},
    digest: `sha256:${id}`,
    run_state: 'running',
    web: { port: 9000, path: '/', https: false },
    created_at: 0,
    ...extra,
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  start.mockClear()
  projects.value = [
    project('media', 'jellyfin'),
    project('uptime-kuma', 'uptime-kuma', { run_state: 'stopped', web: null, digest: null }),
  ]
  earlier.value = [
    app('immich', { state: 'failed', run_state: 'stopped', web: null }),
    app('paperless', { run_state: 'stopped' }),
  ]
})

describe('the installed apps', () => {
  it('show the projects made from apps, then those installed the earlier way', async () => {
    const openApp = vi.fn()
    const wrapper = mount(InstalledSection, {
      global: {
        plugins: [i18n],
        provide: {
          [WINDOW_SECTION as symbol]: {
            section: computed(() => 'installed'),
            navigate: vi.fn(),
            openApp,
          },
        },
      },
    })
    const [made, before] = wrapper.findAll('section')

    expect(made!.text()).toContain('Installed 4')
    const rows = made!.findAll('li')
    expect(rows.map((row) => row.text())).toEqual([
      expect.stringContaining('Running · media'),
      expect.stringContaining('Stopped'),
    ])
    expect(rows[0]!.find('a').attributes('href')).toBe(`http://${window.location.hostname}:9000/`)
    expect(rows[1]!.find('a').exists()).toBe(false)
    expect(rows[1]!.find('img').exists()).toBe(false)
    await rows[0]!.find('button[aria-label="Show in Docker"]').trigger('click')
    expect(openApp).toHaveBeenCalledWith('docker', 'project:media')

    expect(before!.text()).toContain('Installed the earlier way')
    const old = before!.findAll('li')
    expect(old.map((row) => row.text())).toEqual([
      expect.stringContaining('Not installed'),
      expect.stringContaining('Stopped'),
    ])
    expect(old[0]!.findAll('button').some((b) => b.text() === 'Try again')).toBe(false)
    await old[1]!
      .findAll('button')
      .find((b) => b.text() === 'Start')!
      .trigger('click')
    await flushPromises()
    expect(start).toHaveBeenCalledWith('paperless')
  })

  it('say when nothing is installed, and leave out the earlier way when there is none', () => {
    projects.value = []
    earlier.value = []
    const wrapper = mount(InstalledSection, { global: { plugins: [i18n] } })

    expect(wrapper.text()).toContain('No apps installed yet.')
    expect(wrapper.findAll('section')).toHaveLength(1)
    expect(wrapper.find('button[aria-label="Show in Docker"]').exists()).toBe(false)
  })

  it('open the web interface on the address the NAS was reached at', () => {
    const web = { port: 8443, path: '/', https: true }
    expect(webUrl({ web }, 'nas.local')).toBe('https://nas.local:8443/')
    expect(webUrl({ web }, 'fd00::5')).toBe('https://[fd00::5]:8443/')
    expect(webUrl({ web: null })).toBe(null)
  })
})
