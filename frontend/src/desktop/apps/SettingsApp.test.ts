import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import { click, mountApp, page, settle, type } from '@/desktop/testing/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import { i18n } from '@/shared/i18n'
import { WINDOW_SECTION } from '@/shared/lib/window-section'

import SettingsApp from './SettingsApp.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)
vi.mock('@/shared/api/live', async () => ({
  live: (await import('@/shared/test-support/live')).live,
}))

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)
const section = ref('device')

async function open(name: string, permissions?: string[]): Promise<VueWrapper> {
  section.value = name
  const { wrapper } = await mountApp(SettingsApp, {
    permissions,
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
  on('GET', '/api/v1/system/info', {
    hostname: 'nas',
    kernel_release: '6.12.2',
    architecture: 'x86_64',
    cpu_threads: 4,
    memory_total_bytes: 8e9,
    swap_total_bytes: 0,
    uefi: false,
    version: '0.1.1',
    uptime_seconds: 60,
  })
  on('GET', '/api/v1/network', {
    interfaces: [
      {
        name: 'lo',
        kind: 'loopback',
        state: 'up',
        mtu: 65536,
        addresses: [{ address: '127.0.0.1', prefix: 8, scope: 'host', dynamic: false }],
      },
      {
        name: 'eth0',
        kind: 'ethernet',
        state: 'up',
        mtu: 1500,
        addresses: [{ address: '192.168.1.20', prefix: 24, scope: 'global', dynamic: false }],
      },
    ],
    default_routes: [],
    name_servers: [],
  })
  on('GET', '/api/v1/network/setup', { managed: false, pending: null })
  on('GET', '/api/v1/storage/pools', [{ name: 'tank', used_bytes: 1e12, available_bytes: 1e12 }])
  on('PUT', '/api/v1/preferences', (call: { body: unknown }) => call.body)
})

describe('the control panel', () => {
  it('opens on the device and lists only what the account may open', async () => {
    await open('device', ['files.read'])
    expect(page()).toContain('x86_64')
    expect(page()).not.toContain(t('controlPanel.power'))
    expect(page()).not.toContain(t('controlPanel.groupStorage'))
    expect(page()).not.toContain(t('controlPanel.monitor'))
    expect(page()).not.toContain(t('logs.sections.journal'))
    expect(called('GET', '/api/v1/storage/pools')).toHaveLength(0)
  })

  it('holds storage, the resource monitor and the logs as pages of its own', async () => {
    on('GET', '/api/v1/storage/disks', [])
    on('GET', '/api/v1/storage/self-test-plans', [])
    const wrapper = await open('storage-pools')
    expect(page()).toContain(t('controlPanel.groupStorage'))
    expect(page()).toContain(t('controlPanel.groupMonitoring'))
    expect(wrapper.findComponent({ name: 'StoragePage' }).props('page')).toBe('pools')
    // No second frame: the control panel's navigation is the only one.
    expect(wrapper.findAllComponents({ name: 'AppFrame' })).toHaveLength(1)
    section.value = 'storage-disks'
    await settle()
    expect(wrapper.findComponent({ name: 'StoragePage' }).props('page')).toBe('disks')
    section.value = 'monitor'
    await settle()
    expect(wrapper.findComponent({ name: 'ResourceMonitor' }).exists()).toBe(true)
    section.value = 'journal'
    await settle()
    expect(wrapper.findComponent({ name: 'LogsPage' }).props('page')).toBe('journal')
    section.value = 'audit'
    await settle()
    expect(wrapper.findComponent({ name: 'LogsPage' }).props('page')).toBe('audit')
    expect(wrapper.findAllComponents({ name: 'AppFrame' })).toHaveLength(1)
  })

  it('shows the pools on the device page to those who may see storage', async () => {
    await open('device')
    expect(page()).toContain('tank')
  })

  it('chooses the theme and the language', async () => {
    const wrapper = await open('appearance')
    await click(new RegExp(t('appearance.themes.dark')))
    expect(document.querySelector('[role="radio"][aria-checked="true"]')?.textContent).toContain(
      t('appearance.themes.dark'),
    )
    await click(new RegExp(t('appearance.themes.light')))
    const wallpapers = document.querySelectorAll<HTMLButtonElement>('[role="radio"]')
    wallpapers[wallpapers.length - 1]!.click()
    await settle()

    on('GET', '/api/v1/system/time', {
      zone: 'Europe/Berlin',
      network_time: true,
      can_network_time: true,
      synchronized: true,
      servers: [],
      server_in_use: '2.debian.pool.ntp.org',
      zones: ['Etc/UTC', 'Europe/Berlin'],
      now: 1_790_000_000,
    })
    section.value = 'locale'
    await settle()
    expect(page()).toContain('Europe/Berlin')
    wrapper.findComponent({ name: 'PageTabs' }).vm.$emit('update:modelValue', 'language')
    await settle()
    await type('#locale-language', 'de')
    expect(i18n.global.locale.value).toBe('de')
    i18n.global.locale.value = 'en'
  })

  it('restarts and shuts down once confirmed', async () => {
    on('POST', '/api/v1/system/power', (call: { body: { action: string } }) =>
      call.body.action === 'reboot' ? problem(409, 'jobs_running', 'files.copy photos') : null,
    )
    const wrapper = await open('power')
    await click(new RegExp(t('desktop.restartAction')))
    wrapper.findComponent({ name: 'ConfirmDialog' }).vm.$emit('confirm')
    await settle()
    expect(page()).toContain('files.copy photos')

    await click(new RegExp(t('desktop.shutDownAction')))
    wrapper.findComponent({ name: 'ConfirmDialog' }).vm.$emit('confirm')
    await settle()
    expect(called('POST', '/api/v1/system/power').map((call) => call.body)).toEqual([
      { action: 'reboot' },
      { action: 'power_off' },
    ])
  })

  it('shows the own account', async () => {
    await open('account')
    expect(page()).toContain('alice')
    expect(page()).toContain(t('settings.roles.admin'))
  })

  it('opens the other pages', async () => {
    on('GET', '/api/v1/security/settings', problem(503, 'unavailable'))
    on('GET', '/api/v1/notifications/channels', [])
    const wrapper = await open('network')
    expect(page()).toContain(t('controlPanel.networkHelp'))
    for (const name of ['terminal', 'security', 'notifications']) {
      section.value = name
      await settle()
    }
    expect(page()).toContain(t('controlPanel.notificationsHelp'))
    expect(wrapper.findComponent({ name: 'ChannelsSection' }).exists()).toBe(true)
  })
})
