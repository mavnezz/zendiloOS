import { beforeEach, describe, expect, it, vi } from 'vitest'

import { push, sample } from '@/shared/test-support/live'
import { mountApp, page, settle } from '@/shared/test-support/mount'
import { on, reset } from '@/shared/test-support/server'
import type { SystemInfo } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import ResourceMonitor from './ResourceMonitor.vue'
import DeviceInfoPage from './DeviceInfoPage.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)
vi.mock('@/shared/api/live', async () => ({
  live: (await import('@/shared/test-support/live')).live,
}))

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const info: SystemInfo = {
  hostname: 'nas',
  os_name: 'Debian GNU/Linux 13',
  kernel_release: '6.12.2',
  architecture: 'x86_64',
  cpu_model: 'AMD Ryzen 5',
  cpu_cores: 6,
  cpu_threads: 12,
  memory_total_bytes: 8e9,
  swap_total_bytes: 2e9,
  board: 'B550',
  firmware: 'F12',
  uefi: true,
  vendor: 'Acme',
  product: 'NAS 4',
  version: '0.1.1',
  uptime_seconds: 3600,
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/system/info', info)
  on('GET', '/api/v1/system/metrics/history', {
    at: [1, 2],
    cpu_percent: [10, 20],
    memory_used_bytes: [1e9, 2e9],
    received_bytes_per_second: [1, 2],
    sent_bytes_per_second: [1, 2],
    read_bytes_per_second: [1, 2],
    written_bytes_per_second: [1, 2],
  })
})

describe('the resource monitor', () => {
  it('waits for the first sample, then shows every tab', async () => {
    const { wrapper } = await mountApp(ResourceMonitor)
    push('metrics', sample(3))
    push('metrics', sample(4, { cpu_percent: 40 }))
    await settle()
    expect(page()).toContain('40 %')
    expect(page()).toContain('71')
    expect(page()).toContain('AMD Ryzen 5')

    const tabs = wrapper.findComponent({ name: 'PageTabs' })
    for (const tab of ['cpu', 'memory', 'network', 'disks', 'overview']) {
      tabs.vm.$emit('update:modelValue', tab)
      await settle()
    }
    expect(page()).toContain(t('dashboard.tabs.overview'))
    const tiles = [...document.querySelectorAll<HTMLButtonElement>('button')].filter((button) =>
      button.textContent?.includes(t('dashboard.tabs.network')),
    )
    tiles.at(-1)!.click()
    await settle()
    expect(page()).toContain('eth0')
  })

  it('does without temperatures, interfaces and disks', async () => {
    on('GET', '/api/v1/system/info', {
      ...info,
      cpu_model: null,
      cpu_cores: null,
      board: null,
      firmware: null,
      os_name: null,
    })
    on('GET', '/api/v1/system/metrics/history', { at: [] })
    const { wrapper } = await mountApp(ResourceMonitor)
    push(
      'metrics',
      sample(5, {
        temperatures: [],
        network: [],
        disks: [],
        memory: { total_bytes: 0, used_bytes: 0, available_bytes: 0 },
      }),
    )
    await settle()
    expect(page()).toContain('x86_64')
    for (const tab of ['cpu', 'network', 'disks']) {
      wrapper.findComponent({ name: 'PageTabs' }).vm.$emit('update:modelValue', tab)
      await settle()
    }
  })
})

describe('the device page', () => {
  it('shows the machine, its use and the pools', async () => {
    await mountApp(DeviceInfoPage, {
      props: {
        pools: [
          { name: 'tank', used_bytes: 1e12, available_bytes: 3e12 },
          { name: 'odd', used_bytes: null, available_bytes: null },
        ] as never,
      },
    })
    expect(page()).toContain('Acme NAS 4')
    expect(page()).toContain('F12 · UEFI')
    const github = document.querySelector('a')!
    expect([github.textContent?.trim(), github.href, github.target, github.rel]).toEqual([
      t('device.github'),
      'https://github.com/mavnezz/zendiloOS',
      '_blank',
      'noopener noreferrer',
    ])
    push('metrics', sample(6))
    await settle()
    expect(page()).toContain('0.50 · 0.40 · 0.30')
    expect(page()).toContain('tank')
    expect(page()).not.toContain('odd')

    // Side by side as the window allows, read as device, uptime, hardware, storage.
    const grid = document.querySelector<HTMLElement>('[data-testid="device-cards"]')!
    expect(grid.className).toContain('@2xl:grid-cols-2')
    expect(grid.className).toContain('@6xl:grid-cols-3')
    expect(
      [...grid.querySelectorAll(':scope > section h3')].map((h) => h.textContent?.trim()),
    ).toEqual([t('device.device'), t('device.uptime'), t('device.hardware'), t('device.storage')])
  })

  it('does with little', async () => {
    on('GET', '/api/v1/system/info', {
      ...info,
      vendor: null,
      product: null,
      cpu_cores: null,
      swap_total_bytes: 0,
      board: null,
      firmware: null,
      cpu_model: null,
      os_name: null,
    })
    await mountApp(DeviceInfoPage)
    expect(page()).toContain('x86_64')
    expect(page()).toContain('Linux')
  })
})
