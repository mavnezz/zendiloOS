import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import { click, mountApp, page, press, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { Disk, Pool, PoolDevice, RaidArray, Volume } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { WINDOW_SECTION } from '@/shared/lib/window-section'

import StorageApp from './StorageApp.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function disk(name: string, extra: Partial<Disk> = {}): Disk {
  return {
    name,
    model: `Disk ${name}`,
    serial: `S-${name}`,
    size_bytes: 4e12,
    media: 'rotational',
    transport: 'sata',
    usage: 'unused',
    id_link: `/dev/disk/by-id/ata-${name}`,
    partitions: [],
    read_only: false,
    removable: false,
    pool: null,
    array: null,
    wwn: null,
    ...extra,
  }
}

function device(name: string, extra: Partial<PoolDevice> = {}): PoolDevice {
  return {
    name,
    guid: `g-${name}`,
    disk: name,
    state: 'online',
    read_errors: 0,
    write_errors: 0,
    checksum_errors: 0,
    replacing: null,
    ...extra,
  }
}

function pool(name: string, extra: Partial<Pool> = {}): Pool {
  return {
    name,
    guid: `pool-${name}`,
    state: 'online',
    layout: 'mirror',
    layout_name: 'mirror',
    redundancy: 1,
    parity: null,
    size_bytes: 4e12,
    allocated_bytes: 1e12,
    used_bytes: 1e12,
    available_bytes: 3e12,
    data_errors: 0,
    mount_point: `/volume1`,
    volume: 1,
    devices: [device('sda'), device('sdb')],
    scan: null,
    ...extra,
  }
}

const volume: Volume = {
  id: 'v1',
  label: 'tank',
  filesystem: 'zfs',
  devices: ['sda', 'sdb'],
  disks: ['sda', 'sdb'],
  mount_points: ['/volume1'],
  size_bytes: 4e12,
  used_bytes: 1e12,
  available_bytes: 3e12,
  pool: 'tank',
  array: null,
}

const array: RaidArray = {
  name: 'md0',
  level: 'raid1',
  health: 'rebuilding',
  size_bytes: 1e12,
  read_only: false,
  devices_total: 2,
  devices_working: 1,
  members: [
    { device: 'sdc1', index: 0, state: 'active' },
    { device: 'sdd1', index: 1, state: 'replacement' },
  ],
  sync: {
    operation: 'recovery',
    percent: 42.5,
    remaining_seconds: 600,
    speed_bytes_per_second: 1e8,
  },
}

const section = ref('overview')

async function open(name: string, canManage = true): Promise<VueWrapper> {
  section.value = name
  const { wrapper } = await mountApp(StorageApp, {
    props: { canManage },
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
  on('GET', '/api/v1/storage/disks', [
    disk('sda', { usage: 'pool_member', pool: 'tank' }),
    disk('sdb', { usage: 'pool_member', pool: 'tank', media: 'solid_state', transport: 'nvme' }),
    disk('sdc'),
    disk('sdd', {
      usage: 'foreign',
      partitions: [
        { name: 'sdd1', size_bytes: 4e12, filesystem: 'ext4', label: 'old', mount_points: [] },
      ],
    }),
    disk('sde', { usage: 'system', id_link: null }),
  ])
  on('GET', '/api/v1/storage/pools', [
    pool('tank', {
      scan: {
        kind: 'scrub',
        state: 'running',
        percent: 40,
        paused: false,
        bytes_per_second: 2e8,
        seconds_left: 3600,
        errors: 0,
        started_at: 1_790_000_000,
        finished_at: null,
      },
      data_errors: 2,
    }),
    pool('cold', {
      layout: 'raidz',
      layout_name: 'raidz1',
      volume: 2,
      state: 'degraded',
      devices: [
        device('sdf', { state: 'faulted', read_errors: 3 }),
        device('sdg', { replacing: 'new' }),
      ],
      scan: {
        kind: 'resilver',
        state: 'finished',
        percent: 100,
        paused: false,
        errors: 1,
        started_at: 1_790_000_000,
        finished_at: 1_790_003_600,
      },
    }),
    pool('foreign', {
      volume: null,
      scan: {
        kind: 'scrub',
        state: 'canceled',
        percent: 10,
        paused: false,
        errors: 0,
        started_at: 1,
        finished_at: 2,
      },
    }),
  ])
  on('GET', '/api/v1/storage/arrays', [array])
  on('GET', '/api/v1/storage/volumes', [
    volume,
    {
      ...volume,
      id: 'v2',
      label: null,
      size_bytes: null,
      used_bytes: null,
      pool: null,
      mount_points: [],
    },
  ])
  on('GET', '/api/v1/storage/self-test-plans', {
    plans: [{ kind: 'short', frequency: 'weekly', hour: 3, disks: [] }],
  })
  on('GET', '/api/v1/storage/overview', {
    pools: 2,
    disks: 5,
    used_bytes: 1e12,
    available_bytes: 3e12,
    verdict: 'warning',
    concerns: [{ code: 'pool_degraded', params: { pool: 'cold' }, severity: 'warning' }],
  })
})

describe('the storage app', () => {
  it('shows the volumes and their use', async () => {
    await open('volumes')
    expect(page()).toContain('/volume1')
    expect(page()).toContain('zfs')
  })

  it('shows pools with their scans, errors and devices', async () => {
    await open('pools')
    expect(page()).toContain('tank')
    expect(page()).toContain('cold')
    expect(page()).toContain(t('storage.vdevState.degraded'))
    expect(page()).toContain('md0')
    expect(page()).toContain('sdd1')
    expect(page()).toContain(t('storage.stopScrub'))
  })

  it('scrubs and stops scrubbing, and says when that fails', async () => {
    on('POST', '/api/v1/storage/pools/tank/scrub/stop', null)
    on('POST', '/api/v1/storage/pools/cold/scrub', problem(409, 'refused', 'busy'))
    await open('pools')
    await click(new RegExp(t('storage.stopScrub')))
    expect(called('POST', '/api/v1/storage/pools/tank/scrub/stop')).toHaveLength(1)
    await click(new RegExp(t('storage.scrubNow')))
    expect(called('POST', '/api/v1/storage/pools/cold/scrub')).toHaveLength(1)
    expect(page()).toContain('busy')
  })

  it('plans and creates a pool', async () => {
    on('POST', '/api/v1/storage/pools/plan', {
      name: 'fast',
      layout: 'mirror',
      redundancy: 1,
      parity: null,
      capacity_bytes: 4e12,
      mount_point: '/volume3',
      volume: 3,
      token: 'plan-1',
      expires_at: 2_000_000_000,
      disks: [
        {
          name: 'sdc',
          id_link: '/dev/disk/by-id/ata-sdc',
          size_bytes: 4e12,
          model: 'Disk sdc',
          serial: 'S-sdc',
          erases: [],
        },
        {
          name: 'sdd',
          id_link: '/dev/disk/by-id/ata-sdd',
          size_bytes: 4e12,
          model: 'Disk sdd',
          serial: 'S-sdd',
          erases: ['sdd1 (ext4, old)'],
        },
      ],
      warnings: [{ code: 'erases_data' }, { code: 'mixed_sizes', unused_bytes: 1e9 }],
    })
    on('POST', '/api/v1/storage/pools', {
      id: 1,
      kind: 'storage.create_pool',
      state: 'running',
      created_at: 0,
      updated_at: 0,
    })
    await open('pools')
    await click(new RegExp(t('storage.create.button')))
    await type('#pool-name', 'fast')
    for (const box of document.querySelectorAll<HTMLInputElement>(
      '[role="dialog"] input[type="checkbox"]',
    )) {
      box.click()
      await settle()
    }
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/storage/pools/plan')[0]?.body).toEqual({
      name: 'fast',
      layout: 'mirror',
      disks: ['sdc', 'sdd'],
    })
    expect(page()).toContain('sdd1 (ext4, old)')
    await type('#pool-confirm', 'fast')
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/storage/pools')[0]?.body).toEqual({
      token: 'plan-1',
      confirm: 'fast',
    })
    // While the pool is made, the section says so instead of staying empty.
    await settle()
    expect(page()).toContain(t('storage.underway'))
    expect(page()).toContain(`${t('jobs.kinds.storage_create_pool')}`)
  })

  it('goes back from a plan and shows a refused one', async () => {
    on('POST', '/api/v1/storage/pools/plan', problem(422, 'disk_unusable', 'sdc is too small'))
    await open('pools')
    await click(new RegExp(t('storage.create.button')))
    await type('#pool-name', 'fast')
    document.querySelector<HTMLInputElement>('[role="dialog"] input[type="checkbox"]')!.click()
    await settle()
    await submit('[role="dialog"] form')
    expect(page()).toContain('sdc is too small')
  })

  it('destroys a pool once its name is typed', async () => {
    on('POST', '/api/v1/storage/pools/tank/destroy', problem(409, 'pool_in_use', 'share photos'))
    await open('pools')
    document
      .querySelectorAll<HTMLButtonElement>(
        `button[aria-label="${t('storage.destroy.button')}"]`,
      )[0]!
      .click()
    await settle()
    await type('#destroy-confirm', 'tank')
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/storage/pools/tank/destroy')[0]?.body).toEqual({
      confirm: 'tank',
    })
    expect(page()).toContain('share photos')
  })

  it('replaces a disk of a pool', async () => {
    on('POST', '/api/v1/storage/pools/tank/replace/plan', {
      pool: 'tank',
      device: 'g-sda',
      device_name: 'sda',
      token: 'replace-1',
      expires_at: 2_000_000_000,
      disk: {
        name: 'sdc',
        id_link: '/dev/disk/by-id/ata-sdc',
        size_bytes: 4e12,
        model: 'Disk sdc',
        serial: 'S-sdc',
        erases: [],
      },
      warnings: [],
    })
    on('POST', '/api/v1/storage/pools/tank/replace', {
      id: 2,
      kind: 'storage.replace_disk',
      state: 'running',
      created_at: 0,
      updated_at: 0,
    })
    await open('pools')
    await press(t('storage.replaceDisk.button'))
    document.querySelector<HTMLInputElement>('[role="dialog"] input[type="radio"]')!.click()
    await settle()
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/storage/pools/tank/replace/plan')[0]?.body).toEqual({
      device: 'g-sda',
      disk: 'sdc',
    })
    document.querySelector<HTMLInputElement>('[role="dialog"] input[type="checkbox"]')!.click()
    await settle()
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/storage/pools/tank/replace')[0]?.body).toEqual({
      token: 'replace-1',
    })
  })

  it('lists disks, opens one and plans self-tests', async () => {
    on('GET', '/api/v1/storage/disks/sdc/smart', problem(404, 'not_found'))
    const wrapper = await open('disks')
    expect(page()).toContain('Disk sdc')
    expect(page()).toContain(t('storage.selfTestPlans.frequency.weekly'))
    await click(new RegExp(t('storage.selfTestPlans.button')))
    expect(wrapper.findComponent({ name: 'SelfTestPlansDialog' }).props('open')).toBe(true)

    const rows = [...document.querySelectorAll<HTMLElement>('tr')].filter((row) =>
      row.textContent?.includes('Disk sdc'),
    )
    rows[0]!.click()
    await settle()
    expect(wrapper.findComponent({ name: 'DiskDetail' }).exists()).toBe(true)
    section.value = 'overview'
    await settle()
    expect(wrapper.findComponent({ name: 'DiskDetail' }).exists()).toBe(false)
  })

  it('refreshes everything on request', async () => {
    await open('overview')
    const before = called('GET', '/api/v1/storage/overview').length
    await click(new RegExp(t('storage.refresh')))
    expect(called('GET', '/api/v1/storage/overview').length).toBeGreaterThan(before)
  })

  it('shows nothing to change to those who may only look', async () => {
    await open('pools', false)
    expect(page()).not.toContain(t('storage.scrubNow'))
    expect(document.querySelector(`button[aria-label="${t('storage.destroy.button')}"]`)).toBeNull()
  })
})
