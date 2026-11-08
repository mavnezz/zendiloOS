import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { click, labelled, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type {
  Disk,
  Enclosure,
  EnclosureModel,
  InventoryDisk,
  PlacedSlot,
  Pool,
} from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import StoragePage from './StoragePage.vue'

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
    partitions: [],
    read_only: false,
    removable: false,
    id_link: `ata-${name}`,
    id_path: `pci-0000:05:00.0-ata-${'abcd'.indexOf(name.at(-1) ?? '') + 1}.0`,
    ...extra,
  }
}

const port = (n: number) => ({ kind: 'port' as const, value: `pci-0000:05:00.0-ata-${n}` })

const dxp: EnclosureModel = {
  id: 'ugreen-dxp4800-plus',
  vendor: 'UGREEN',
  name: 'DXP4800 Plus',
  dmi_vendor: 'UGREEN',
  dmi_product: 'DXP4800 Plus',
  verified: false,
  notes: 'To confirm: the order of bays 1 to 4.',
  chassis: {
    style: 'tower',
    panel: { side: 'left', power_button: true, leds: ['power', 'network'] },
  },
  groups: [
    {
      id: 'bays',
      name: 'Front bays',
      rows: 1,
      columns: 3,
      orientation: 'vertical',
      form: 'inch_3_5',
      numbering: 'left_to_right',
      slots: [1, 2, 3].map((n) => ({
        id: `bay${n}`,
        label: `Bay ${n}`,
        row: 0,
        column: n - 1,
        binding: port(n),
      })),
    },
  ],
}

function placed(n: number, extra: Partial<PlacedSlot> = {}): PlacedSlot {
  return {
    id: `bay${n}`,
    label: `Bay ${n}`,
    row: 0,
    column: n - 1,
    binding: port(n),
    overridden: false,
    state: 'empty',
    ...extra,
  }
}

function chosen(extra: Partial<Enclosure> = {}): Enclosure {
  return {
    machine: { vendor: 'UGREEN', product: 'DXP4800 Plus' },
    detected: dxp,
    source: 'model',
    model: dxp,
    chassis: dxp.chassis,
    groups: [
      {
        ...dxp.groups[0]!,
        slots: [
          placed(1, { state: 'occupied', disk: 'sda', health: 'good' }),
          placed(2, {
            state: 'missing',
            binding: { kind: 'serial', value: 'GONE' },
            overridden: true,
          }),
          placed(3, { state: 'occupied', disk: 'sdc', health: 'failing' }),
        ],
      },
    ],
    unplaced: ['sdd'],
    ...extra,
  }
}

const notSetUp: Enclosure = {
  machine: { vendor: 'UGREEN', product: 'DXP4800 Plus' },
  detected: dxp,
  source: null,
  chassis: { style: 'plain', panel: null },
  groups: [],
  unplaced: [],
}

const stored: InventoryDisk = {
  serial: 'OLD-1',
  model: 'Old disk',
  size_bytes: 2e12,
  first_seen: 1_700_000_000,
  last_seen: 1_750_000_000,
  warranty_until: '2020-01-01',
  disk: null,
}

async function open(page: 'overview' | 'disks' | 'pools', canManage = true): Promise<VueWrapper> {
  const { wrapper } = await mountApp(StoragePage, { props: { page, canManage } })
  return wrapper
}

function select(selector: string): HTMLSelectElement {
  const found = document.body.querySelector<HTMLSelectElement>(selector)
  if (!found) throw new Error(`no ${selector}`)
  return found
}

async function choose(element: HTMLSelectElement, value: string): Promise<void> {
  element.value = value
  element.dispatchEvent(new Event('change'))
  await settle()
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/storage/disks', [disk('sda'), disk('sdb'), disk('sdc'), disk('sdd')])
  on('GET', '/api/v1/storage/self-test-plans', { plans: [] })
  on('GET', '/api/v1/storage/enclosure/models', [dxp])
  on('GET', '/api/v1/storage/locate', [])
  on('GET', '/api/v1/storage/inventory', [stored])
  on('GET', '/api/v1/storage/enclosure', chosen())
})

describe('the enclosure on the disks page', () => {
  it('offers the detected model in one click while nothing is set up', async () => {
    on('GET', '/api/v1/storage/enclosure', notSetUp)
    on('PUT', '/api/v1/storage/enclosure', chosen())
    await open('disks')
    expect(page()).toContain(t('storage.enclosure.callToAction'))
    expect(page()).toContain(t('storage.enclosure.setUp'))
    await click(t('storage.enclosure.useModel', { model: 'UGREEN DXP4800 Plus' }))
    expect(called('PUT', '/api/v1/storage/enclosure')[0]?.body).toEqual({
      source: 'model',
      model: 'ugreen-dxp4800-plus',
    })
  })

  it('starts a layout of one’s own where no model is known', async () => {
    on('GET', '/api/v1/storage/enclosure', { ...notSetUp, detected: null })
    on('GET', '/api/v1/storage/enclosure/models', [])
    on('PUT', '/api/v1/storage/enclosure', chosen({ source: 'custom', model: null }))
    await open('disks')
    await click(t('storage.enclosure.setUp'))
    expect(select('#enclosure-source').value).toBe('custom')
    await submit('#enclosure-form')
    const body = called('PUT', '/api/v1/storage/enclosure')[0]?.body as {
      groups: { name: string; slots: unknown[] }[]
    }
    expect(body.groups[0]).toMatchObject({ name: 'Group 1' })
    expect(body.groups[0]!.slots).toHaveLength(4)
  })

  it('says why the model could not be chosen', async () => {
    on('GET', '/api/v1/storage/enclosure', notSetUp)
    on('PUT', '/api/v1/storage/enclosure', problem(422, 'invalid_input', 'there is no model'))
    await open('disks')
    await click(t('storage.enclosure.useModel', { model: 'UGREEN DXP4800 Plus' }))
    expect(page()).toContain('there is no model')
  })

  it('draws each disk in its slot, empty and missing ones, and opens a disk', async () => {
    on('GET', '/api/v1/storage/disks/sda/smart', {
      status: 'available',
      health: 'good',
      temperature_celsius: 36,
      attributes: [],
      findings: [],
      self_tests_supported: true,
    })
    const wrapper = await open('disks')
    const tiles = [...document.querySelectorAll<HTMLElement>('[data-tone]')]
    expect(tiles.map((tile) => tile.dataset.tone)).toEqual(['good', 'missing', 'failing'])
    expect(tiles[0]!.textContent).toContain('Disk sda')
    expect(tiles[0]!.textContent).toContain('36 °C')
    expect(tiles[1]!.textContent).toContain(t('storage.enclosure.missing'))
    expect(page()).toContain(t('storage.enclosure.notPlacedHint'))
    // The slot column of the table.
    const row = [...document.querySelectorAll('tr')].find((r) =>
      r.textContent?.includes('Disk sdc'),
    )
    expect(row?.textContent).toContain('Bay 3')

    labelled(t('storage.enclosure.tile', { slot: 'Bay 3', disk: 'sdc', state: 'failing' })).click()
    await settle()
    expect(wrapper.findComponent({ name: 'DiskDetail' }).props('name')).toBe('sdc')
    expect(page()).toContain('Bay 3, Front bays')
  })

  it('opens a disk not placed from the line below', async () => {
    const wrapper = await open('disks')
    await click('sdd')
    expect(wrapper.findComponent({ name: 'DiskDetail' }).props('name')).toBe('sdd')
  })

  it('overrides a slot of the model and saves', async () => {
    on('PUT', '/api/v1/storage/enclosure', chosen())
    await open('disks')
    await click(t('storage.enclosure.edit'))
    expect(page()).toContain(t('storage.enclosure.dialog.unverified'))
    expect(page()).toContain('To confirm: the order of bays 1 to 4.')
    expect(select('#enclosure-source').value).toBe('ugreen-dxp4800-plus')

    // Bay 1 by sdd's serial number; bay 2 back to the model's port.
    await choose(select('[role="dialog"] select[aria-label="Bay 1"]'), 'serial:S-sdd')
    await choose(select('[role="dialog"] select[aria-label="Bay 2"]'), 'model')
    await submit('#enclosure-form')
    expect(called('PUT', '/api/v1/storage/enclosure')[0]?.body).toEqual({
      source: 'model',
      model: 'ugreen-dxp4800-plus',
      overrides: [{ slot: 'bay1', binding: { kind: 'serial', value: 'S-sdd' } }],
    })
  })

  it('puts a disk not placed into a slot, by port', async () => {
    on('PUT', '/api/v1/storage/enclosure', chosen())
    await open('disks')
    await click(t('storage.enclosure.edit'))
    await choose(
      select(`[aria-label="${t('storage.enclosure.dialog.putInto', { disk: 'sdd' })}"]`),
      'bay2',
    )
    await submit('#enclosure-form')
    expect(called('PUT', '/api/v1/storage/enclosure')[0]?.body).toEqual({
      source: 'model',
      model: 'ugreen-dxp4800-plus',
      overrides: [{ slot: 'bay2', binding: { kind: 'port', value: 'pci-0000:05:00.0-ata-4.0' } }],
    })
  })

  it('makes a layout of one’s own from the model, resized and renamed', async () => {
    on('PUT', '/api/v1/storage/enclosure', chosen({ source: 'custom', model: null }))
    await open('disks')
    await click(t('storage.enclosure.edit'))
    await click(t('storage.enclosure.dialog.fromModel'))
    expect(select('#enclosure-source').value).toBe('custom')
    const rows = document.querySelector<HTMLInputElement>('[role="dialog"] input[type="number"]')!
    rows.value = '2'
    rows.dispatchEvent(new Event('change'))
    await settle()
    await type('[role="dialog"] fieldset input:not([type])', 'Front')
    await click(new RegExp(t('storage.enclosure.dialog.addGroup')))
    await submit('#enclosure-form')
    const body = called('PUT', '/api/v1/storage/enclosure')[0]?.body as {
      source: string
      groups: { name: string; rows: number; slots: { id: string; binding: unknown }[] }[]
    }
    expect(body.source).toBe('custom')
    expect(body.groups).toHaveLength(2)
    expect(body.groups[0]).toMatchObject({ name: 'Front', rows: 2 })
    expect(body.groups[0]!.slots).toHaveLength(6)
    expect(body.groups[0]!.slots[0]).toEqual({ id: 'bay1', label: 'Bay 1', binding: port(1) })
    expect(body.groups[1]!.slots).toHaveLength(4)
  })

  it('keeps a layout from being saved while a slot has no label', async () => {
    await open('disks')
    await click(t('storage.enclosure.edit'))
    await choose(select('#enclosure-source'), 'custom')
    await type('[role="dialog"] ol input', '')
    expect(page()).toContain(t('storage.enclosure.dialog.problems.label'))
    labelled(t('storage.enclosure.dialog.removeGroup')).click()
    await settle()
    expect(page()).toContain(t('storage.enclosure.dialog.problems.groups'))
  })

  it('removes the enclosure', async () => {
    on('DELETE', '/api/v1/storage/enclosure', notSetUp)
    await open('disks')
    await click(t('storage.enclosure.edit'))
    await click(t('storage.enclosure.dialog.remove'))
    expect(called('DELETE', '/api/v1/storage/enclosure')).toHaveLength(1)
  })

  it('lists disks no longer present and forgets them', async () => {
    on('DELETE', '/api/v1/storage/inventory/OLD-1', null)
    await open('disks')
    expect(page()).toContain(t('storage.inventory.stored'))
    expect(page()).toContain(t('storage.inventory.warrantyEnded'))
    labelled(t('storage.inventory.forget', { serial: 'OLD-1' })).click()
    await settle()
    expect(called('DELETE', '/api/v1/storage/inventory/OLD-1')).toHaveLength(1)
  })

  it('offers no changes to those who may only look', async () => {
    await open('disks', false)
    expect(page()).not.toContain(t('storage.enclosure.edit'))
    expect(
      document.querySelector(
        `button[aria-label="${t('storage.inventory.forget', { serial: 'OLD-1' })}"]`,
      ),
    ).toBeNull()
  })
})

describe('the enclosure elsewhere', () => {
  it('is drawn small on the overview, and a slot opens its disk', async () => {
    on('GET', '/api/v1/storage/overview', {
      pools: 1,
      disks: 4,
      used_bytes: 1,
      available_bytes: 1,
      verdict: null,
      concerns: [],
    })
    on('GET', '/api/v1/storage/pools', [])
    const wrapper = await open('overview')
    expect(page()).toContain(t('storage.enclosure.title'))
    labelled(t('storage.enclosure.tile', { slot: 'Bay 1', disk: 'sda', state: 'healthy' })).click()
    await settle()
    expect(wrapper.findComponent({ name: 'DiskDetail' }).props('name')).toBe('sda')
  })

  it('names the slot of the disk being replaced', async () => {
    const tank: Pool = {
      name: 'tank',
      guid: 'pool-tank',
      state: 'degraded',
      layout: 'mirror',
      layout_name: 'mirror',
      redundancy: 1,
      parity: null,
      size_bytes: 4e12,
      allocated_bytes: 1e12,
      used_bytes: 1e12,
      available_bytes: 3e12,
      data_errors: 0,
      mount_point: '/volume1',
      volume: 1,
      devices: [
        {
          name: 'sdc',
          guid: 'g-sdc',
          disk: 'sdc',
          state: 'faulted',
          read_errors: 9,
          write_errors: 0,
          checksum_errors: 0,
          replacing: null,
        },
      ],
      scan: null,
    }
    on('GET', '/api/v1/storage/pools', [tank])
    on('GET', '/api/v1/storage/arrays', [])
    await open('pools')
    labelled(t('storage.replaceDisk.button')).click()
    await settle()
    expect(document.querySelector('[data-testid="replace-slot"]')?.textContent).toContain(
      t('storage.enclosure.inSlot', { slot: 'Bay 3' }),
    )
  })
})
