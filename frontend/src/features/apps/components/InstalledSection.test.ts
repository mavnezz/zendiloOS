import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { InstalledApp } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import { webUrl } from '../api'
import InstalledSection from './InstalledSection.vue'

const update = vi.fn<(id: string, confirm: boolean) => Promise<void>>(() => Promise.resolve())
const retry = vi.fn<(id: string) => Promise<void>>(() => Promise.resolve())
const apps = ref<InstalledApp[]>([])

vi.mock('../api', async (original) => ({
  ...(await original<typeof import('../api')>()),
  useInstalled: () => ({ data: apps, error: ref(null), isPending: ref(false), refetch: vi.fn() }),
  useInstallActions: () => ({
    update: (id: string, confirm: boolean) => update(id, confirm),
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
    permissions: { devices: [], capabilities: [], host_network: false, docker_socket: false },
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
  update.mockClear()
  retry.mockClear()
  apps.value = [
    app('jellyfin', { update: { version: '1.1' } }),
    app('immich', { state: 'failed', run_state: 'stopped', web: null }),
  ]
})

describe('the installed apps', () => {
  it('put newer versions first and offer what fits', async () => {
    const wrapper = mount(InstalledSection, { global: { plugins: [i18n] } })
    const [updatable, installed] = wrapper.findAll('section')

    expect(updatable!.text()).toContain('Update available')
    expect(updatable!.text()).toContain('Version 1.1 available')
    expect(updatable!.text()).not.toContain('Immich')
    const rows = installed!.findAll('li')
    expect(rows.map((row) => row.text())).toEqual([
      expect.stringContaining('Not installed'),
      expect.stringContaining('Running'),
    ])
    expect(rows[1]!.find('a').attributes('href')).toBe(
      `http://${window.location.hostname}:8096/web`,
    )
    expect(rows[0]!.find('a').exists()).toBe(false)

    await rows[0]!
      .findAll('button')
      .find((b) => b.text() === 'Try again')!
      .trigger('click')
    await updatable!
      .findAll('button')
      .find((b) => b.text() === 'Update all')!
      .trigger('click')
    await flushPromises()
    expect(retry).toHaveBeenCalledWith('immich')
    expect(update).toHaveBeenCalledWith('jellyfin', null)
  })

  it('open the web interface on the address the NAS was reached at', () => {
    const web = { port: 8443, path: '/', https: true }
    expect(webUrl({ web }, 'nas.local')).toBe('https://nas.local:8443/')
    expect(webUrl({ web }, 'fd00::5')).toBe('https://[fd00::5]:8443/')
    expect(webUrl({ web: null })).toBe(null)
  })
})
