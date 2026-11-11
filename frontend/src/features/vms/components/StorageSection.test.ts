import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { VmLocation, VmPlugin } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { formatBytes, formatPercent } from '@/shared/lib/format'
import { click, mountApp, page, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import StorageSection from './StorageSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}, plural?: number) =>
  plural === undefined ? i18n.global.t(key, values) : i18n.global.t(key, values, plural)
const GIB = 1024 ** 3

const pool = (name: string, volume: number) => ({
  pool: name,
  volume,
  health: 'online',
  used_bytes: 10 * GIB,
  available_bytes: 90 * GIB,
})

const PLUGIN: VmPlugin = {
  hardware: { kvm: true, cpu_virtualization: 'vmx', iommu: false },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  locations: [
    { pool: 'tank', volume: 1 },
    { pool: 'cold', volume: 2 },
  ],
  pools: [pool('tank', 1), pool('cold', 2), pool('fast', 3)],
  cores: 4,
  memory_bytes: 8 * GIB,
  bridges: [],
  internal_networks: [],
  cards: [],
  image_upload_max_bytes: 32 * GIB,
}

const LOCATIONS: VmLocation[] = [
  {
    pool: 'tank',
    volume: 1,
    library: true,
    health: 'online',
    pool_used_bytes: 10 * GIB,
    available_bytes: 90 * GIB,
    used_bytes: 6 * GIB,
    images_bytes: 2 * GIB,
    images: 2,
    vms: [
      {
        id: '0a1b2c3d4e5f',
        name: 'web',
        disks: [0, 2],
        used_bytes: 3 * GIB,
        referenced_bytes: 2 * GIB,
        size_bytes: 40 * GIB,
      },
    ],
  },
  {
    pool: 'cold',
    volume: 2,
    library: false,
    health: 'degraded',
    pool_used_bytes: null,
    available_bytes: null,
    used_bytes: null,
    images_bytes: null,
    images: 0,
    vms: [],
  },
]

beforeEach(() => {
  reset()
  on('GET', '/api/v1/vms/locations', LOCATIONS)
})

function location(pool: string): string {
  return document.querySelector(`[data-location="${pool}"]`)?.textContent ?? ''
}

describe('the VMs’ storage', () => {
  it('shows each location with its pool, what lies on it and each VM’s share', async () => {
    await mountApp(StorageSection, { props: { plugin: PLUGIN } })
    const tank = location('tank')
    expect(tank).toContain(t('vms.storage.pool', { pool: 'tank' }))
    expect(tank).toContain(t('vms.storage.library'))
    expect(tank).toContain(t('vms.storage.health.online'))
    expect(tank).toContain(
      t('vms.storage.used', { used: formatBytes(10 * GIB), free: formatBytes(90 * GIB) }),
    )
    expect(tank).toContain(t('vms.storage.images', { n: 2, size: formatBytes(2 * GIB) }, 2))
    expect(tank).toContain('web')
    expect(tank).toContain(t('vms.storage.disks', { disks: '1, 3' }, 2))
    expect(tank).toContain(
      t('vms.storage.takes', { used: formatBytes(3 * GIB), size: formatBytes(40 * GIB) }),
    )
    expect(tank).toContain(t('vms.storage.share', { percent: formatPercent(50) }))
    const cold = location('cold')
    expect(cold).toContain(t('vms.storage.health.degraded'))
    expect(cold).toContain(t('vms.storage.noVms'))
    // In use, or the library's while there are others: not removed.
    const remove = (pool: string) =>
      document.querySelector<HTMLButtonElement>(
        `button[aria-label="${t('vms.storage.removeLabel', { pool })}"]`,
      )!
    expect(remove('tank').disabled).toBe(true)
    expect(remove('cold').disabled).toBe(false)
  })

  it('adds a location on a pool that holds none', async () => {
    on('POST', '/api/v1/vms/locations', PLUGIN)
    await mountApp(StorageSection, { props: { plugin: PLUGIN } })
    await click(new RegExp(t('vms.storage.add')))
    const options = [...document.querySelectorAll<HTMLOptionElement>('#vm-location-pool option')]
    expect(options.map((option) => option.value)).toEqual(['fast'])
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/vms/locations')[0]?.body).toEqual({ pool: 'fast' })
  })

  it('removes a location only with its pool’s name typed again, and says why not', async () => {
    on('POST', '/api/v1/vms/locations/cold/delete', problem(409, 'location_in_use', 'web'))
    await mountApp(StorageSection, { props: { plugin: PLUGIN } })
    document
      .querySelector<HTMLButtonElement>(
        `button[aria-label="${t('vms.storage.removeLabel', { pool: 'cold' })}"]`,
      )!
      .click()
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(page()).toContain(t('vms.storage.removeDialog.title', { pool: 'cold' }))
    const confirm = () =>
      document.querySelector<HTMLButtonElement>('[role="dialog"] button[type="submit"]')!
    expect(confirm().disabled).toBe(true)
    await type('#vm-location-confirm', 'Cold')
    expect(confirm().disabled).toBe(true)
    await type('#vm-location-confirm', 'cold')
    expect(confirm().disabled).toBe(false)
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/vms/locations/cold/delete')[0]?.body).toEqual({
      confirm: 'cold',
    })
    expect(document.querySelector('[data-testid="vm-storage-failure"]')?.textContent).toContain(
      'web',
    )
  })
})
