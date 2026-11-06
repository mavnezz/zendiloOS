import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { InstalledApp } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import { webUrl } from '../api'
import InstalledSection from './InstalledSection.vue'

const stop = vi.fn<(id: string) => Promise<void>>(() => Promise.resolve())
const retry = vi.fn<(id: string) => Promise<void>>(() => Promise.resolve())
const apps = ref<InstalledApp[]>([])

vi.mock('../api', async (original) => ({
  ...(await original<typeof import('../api')>()),
  useInstalled: () => ({ data: apps, error: ref(null), isPending: ref(false), refetch: vi.fn() }),
  useInstallActions: () => ({
    stop: (id: string) => stop(id),
    retry: (id: string) => retry(id),
  }),
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
    permissions: { devices: [], capabilities: [], host_network: false },
    update: null,
    pool: 'tank',
    installed_at: 0,
    updated_at: 0,
    ...extra,
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  i18n.global.locale.value = 'en'
  stop.mockClear()
  retry.mockClear()
  apps.value = [
    app('jellyfin', { update: '1.1' }),
    app('immich', { state: 'failed', run_state: 'stopped', web: null }),
  ]
})

describe('the installed apps', () => {
  it('show how they run and offer what fits', async () => {
    const wrapper = mount(InstalledSection, { global: { plugins: [i18n] } })
    const [jellyfin, immich] = wrapper.findAll('li')
    expect(jellyfin!.text()).toContain('Running')
    expect(jellyfin!.text()).toContain('Version 1.1 available')
    expect(jellyfin!.find('a').attributes('href')).toBe(
      `http://${window.location.hostname}:8096/web`,
    )
    expect(immich!.text()).toContain('Not installed')
    expect(immich!.find('a').exists()).toBe(false)

    await jellyfin!
      .findAll('button')
      .find((b) => b.text() === 'Stop')!
      .trigger('click')
    await immich!
      .findAll('button')
      .find((b) => b.text() === 'Try again')!
      .trigger('click')
    await flushPromises()
    expect(stop).toHaveBeenCalledWith('jellyfin')
    expect(retry).toHaveBeenCalledWith('immich')
  })

  it('open the web interface on the address the NAS was reached at', () => {
    const web = { port: 8443, path: '/', https: true }
    expect(webUrl({ web }, 'nas.local')).toBe('https://nas.local:8443/')
    expect(webUrl({ web }, 'fd00::5')).toBe('https://[fd00::5]:8443/')
    expect(webUrl({ web: null })).toBe(null)
  })
})
