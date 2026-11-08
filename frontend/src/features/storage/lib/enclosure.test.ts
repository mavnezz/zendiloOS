import { describe, expect, it } from 'vitest'

import type { Disk, Enclosure, EnclosureModel, PlacedSlot } from '@/shared/api/types'

import {
  bindSlot,
  bindingFor,
  boundDisk,
  cell,
  draftOf,
  formatCountdown,
  groupsOf,
  idsOf,
  modelBinding,
  newGroup,
  overrideSlot,
  problemsOf,
  requestOf,
  resize,
  sameBinding,
  secondsLeft,
  slotOf,
  slotTone,
} from './enclosure'

const port = (value: string) => ({ kind: 'port' as const, value })
const serial = (value: string) => ({ kind: 'serial' as const, value })

const model: EnclosureModel = {
  id: 'acme-2',
  vendor: 'ACME',
  name: 'Two',
  dmi_vendor: 'ACME',
  dmi_product: 'Two',
  verified: false,
  notes: '',
  chassis: { style: 'tower', panel: null },
  groups: [
    {
      id: 'bays',
      name: 'Bays',
      rows: 1,
      columns: 2,
      orientation: 'vertical',
      form: 'inch_3_5',
      numbering: 'left_to_right',
      slots: [
        { id: 'bay1', label: 'Bay 1', row: 0, column: 0, binding: port('pci-1-ata-1') },
        { id: 'bay2', label: 'Bay 2', row: 0, column: 1, binding: port('pci-1-ata-2') },
      ],
    },
  ],
}

function placed(id: string, extra: Partial<PlacedSlot> = {}): PlacedSlot {
  return { id, label: id, row: 0, column: 0, overridden: false, state: 'empty', ...extra }
}

function enclosure(extra: Partial<Enclosure> = {}): Enclosure {
  return {
    machine: { vendor: 'ACME', product: 'Two' },
    chassis: { style: 'plain', panel: null },
    groups: [],
    unplaced: [],
    ...extra,
  }
}

function disk(name: string, extra: Partial<Disk> = {}): Disk {
  return {
    name,
    size_bytes: 1,
    transport: 'sata',
    media: 'rotational',
    usage: 'unused',
    removable: false,
    read_only: false,
    partitions: [],
    serial: `S-${name}`,
    id_path: `pci-1-ata-${name.at(-1) === 'a' ? 1 : 2}.0`,
    ...extra,
  }
}

describe('a slot’s tile', () => {
  it('is coloured by the disk’s report, the monitor’s verdict, or what is not there', () => {
    expect(slotTone(placed('a', { state: 'empty' }))).toBe('empty')
    expect(slotTone(placed('a', { state: 'missing' }))).toBe('missing')
    const occupied = placed('a', { state: 'occupied', health: 'warning' })
    expect(slotTone(occupied)).toBe('warning')
    expect(slotTone(placed('a', { state: 'occupied' }))).toBe('unknown')
    expect(
      slotTone(occupied, {
        status: 'available',
        health: 'failing',
        attributes: [],
        findings: [],
        self_tests_supported: true,
      }),
    ).toBe('failing')
    expect(
      slotTone(occupied, {
        status: 'standby',
        attributes: [],
        findings: [],
        self_tests_supported: false,
      }),
    ).toBe('asleep')
  })

  it('is found for a disk, with its group', () => {
    const groups = [
      { ...model.groups[0]!, slots: [placed('bay1', { label: 'Bay 1', disk: 'sdc' })] },
    ]
    expect(slotOf(enclosure({ groups }), 'sdc')).toEqual({
      label: 'Bay 1',
      group: 'Bays',
      unconfirmed: false,
    })
    expect(slotOf(enclosure({ groups }), 'sdz')).toBeNull()
    expect(slotOf(undefined, 'sdc')).toBeNull()
  })

  it('is not confirmed while a model nobody checked binds it', () => {
    const unchecked = { ...model, verified: false }
    const slots = [
      placed('bay1', { label: 'Bay 1', disk: 'sdc' }),
      placed('bay2', { label: 'Bay 2', disk: 'sdd', overridden: true }),
    ]
    const groups = [{ ...model.groups[0]!, slots }]
    const chosen = enclosure({ source: 'model', model: unchecked, groups })
    expect(slotOf(chosen, 'sdc')?.unconfirmed).toBe(true)
    expect(slotOf(chosen, 'sdd')?.unconfirmed, 'bound by hand').toBe(false)
    const checked = enclosure({ source: 'model', model: { ...model, verified: true }, groups })
    expect(slotOf(checked, 'sdc')?.unconfirmed).toBe(false)
    expect(slotOf(enclosure({ source: 'custom', groups }), 'sdc')?.unconfirmed).toBe(false)
  })
})

describe('slots in their grid', () => {
  it('are placed as the backend places them', () => {
    const cells = (numbering: Parameters<typeof cell>[3]) =>
      [0, 1, 2, 3, 4, 5].map((index) => cell(index, 2, 3, numbering))
    expect(cells('left_to_right').map((c) => [c.row, c.column])).toEqual([
      [0, 0],
      [0, 1],
      [0, 2],
      [1, 0],
      [1, 1],
      [1, 2],
    ])
    expect(cells('right_to_left').map((c) => [c.row, c.column])).toEqual([
      [0, 2],
      [0, 1],
      [0, 0],
      [1, 2],
      [1, 1],
      [1, 0],
    ])
    expect(cells('top_to_bottom').map((c) => [c.row, c.column])).toEqual([
      [0, 0],
      [1, 0],
      [0, 1],
      [1, 1],
      [0, 2],
      [1, 2],
    ])
    expect(cells('bottom_to_top').map((c) => [c.row, c.column])).toEqual([
      [1, 0],
      [0, 0],
      [1, 1],
      [0, 1],
      [1, 2],
      [0, 2],
    ])
    expect(cell(0, 0, 0, 'left_to_right')).toEqual({ row: 0, column: 0 })
  })
})

describe('bindings', () => {
  it('are by port, by serial number on USB or without a port', () => {
    expect(bindingFor(disk('sda'))).toEqual(port('pci-1-ata-1.0'))
    expect(bindingFor(disk('sdf', { transport: 'usb' }))).toEqual(serial('S-sdf'))
    expect(bindingFor(disk('sdg', { id_path: null }))).toEqual(serial('S-sdg'))
    expect(bindingFor(disk('sdh', { transport: 'usb', serial: null }))).toEqual(
      port('pci-1-ata-2.0'),
    )
    expect(bindingFor(disk('vda', { id_path: null, serial: null }))).toBeNull()
  })

  it('name the same port in both spellings', () => {
    expect(sameBinding(port('pci-1-ata-1'), port('pci-1-ata-1.0'))).toBe(true)
    expect(sameBinding(port('pci-1-ata-1.0-x'), port('pci-1-ata-1-x'))).toBe(true)
    expect(sameBinding(port('pci-1-ata-1'), port('pci-1-ata-1.1'))).toBe(false)
    expect(sameBinding(serial('A'), serial('A'))).toBe(true)
    expect(sameBinding(serial('A'), port('A'))).toBe(false)
    expect(sameBinding(null, serial('A'))).toBe(false)
  })

  it('lead to the disk present', () => {
    const disks = [disk('sda'), disk('sdb')]
    expect(boundDisk(port('pci-1-ata-1'), disks)?.name).toBe('sda')
    expect(boundDisk(serial('S-sdb'), disks)?.name).toBe('sdb')
    expect(boundDisk(serial('gone'), disks)).toBeUndefined()
    expect(boundDisk(null, disks)).toBeUndefined()
    expect(boundDisk(port('pci-1-ata-1'), [disk('sdc', { id_path: null })])).toBeUndefined()
  })
})

describe('the draft of the dialog', () => {
  it('starts from what is chosen, or the detected model', () => {
    expect(draftOf(enclosure({ detected: model }), [])).toEqual({
      source: 'model',
      model: 'acme-2',
      overrides: {},
    })
    expect(draftOf(enclosure(), [model])).toMatchObject({ source: 'model', model: 'acme-2' })
    const blank = draftOf(enclosure(), [])
    expect(blank.source).toBe('custom')

    const chosen = enclosure({
      source: 'model',
      model,
      groups: [
        {
          ...model.groups[0]!,
          slots: [
            placed('bay1', { overridden: true, binding: serial('X') }),
            placed('bay2', { overridden: true, binding: null }),
            placed('bay3'),
          ],
        },
      ],
    })
    expect(draftOf(chosen, [model])).toEqual({
      source: 'model',
      model: 'acme-2',
      overrides: { bay1: serial('X'), bay2: null },
    })

    const own = enclosure({
      source: 'custom',
      groups: [{ ...model.groups[0]!, slots: [placed('a', { label: 'A', binding: port('p') })] }],
    })
    const draft = draftOf(own, [model])
    expect(draft).toMatchObject({ source: 'custom' })
    expect(draft.source === 'custom' && draft.groups[0]!.slots).toEqual([
      { id: 'a', label: 'A', binding: port('p') },
    ])
  })

  it('takes a model’s groups as a layout of one’s own', () => {
    const groups = groupsOf(model)
    expect(groups[0]!.slots.map((s) => s.binding)).toEqual([
      port('pci-1-ata-1'),
      port('pci-1-ata-2'),
    ])
    expect(idsOf(groups)).toEqual(['bays', 'bay1', 'bay2'])
  })

  it('adds groups and resizes them with unique ids', () => {
    const first = newGroup([])
    expect(first).toMatchObject({ id: 'group1', rows: 1, columns: 4, form: 'inch_3_5' })
    expect(first.slots.map((s) => [s.id, s.label])).toEqual([
      ['group1-1', '1'],
      ['group1-2', '2'],
      ['group1-3', '3'],
      ['group1-4', '4'],
    ])
    const second = newGroup([first], 'M.2')
    expect(second.id).toBe('group2')
    expect(second.name).toBe('M.2')

    const bigger = resize(
      { ...first, slots: [{ ...first.slots[0]!, label: 'Top' }, ...first.slots.slice(1)] },
      2,
      3,
    )
    expect(bigger.slots).toHaveLength(6)
    expect(bigger.slots[0]!.label).toBe('Top')
    expect(bigger.slots[5]).toEqual({ id: 'group1-6', label: '6', binding: null })
    expect(resize(first, 1, 2).slots.map((s) => s.id)).toEqual(['group1-1', 'group1-2'])
    // At most 60 slots; nonsense becomes one.
    expect(resize(first, 10, 10).columns).toBe(6)
    expect(resize(first, Number.NaN, 0)).toMatchObject({ rows: 1, columns: 1 })
    const taken = resize(first, 1, 5, new Set(['group1-5']))
    expect(taken.slots[4]!.id).toBe('group1-6')
  })

  it('binds a disk to one slot only', () => {
    const groups = groupsOf(model)
    const moved = bindSlot(groups, 'bay2', port('pci-1-ata-1.0'))
    expect(moved[0]!.slots.map((s) => s.binding)).toEqual([null, port('pci-1-ata-1.0')])
    const cleared = bindSlot(moved, 'bay2', null)
    expect(cleared[0]!.slots[1]!.binding).toBeNull()
  })

  it('overrides a model’s slots, the disk taken from the slot it was in', () => {
    expect(modelBinding(model, 'bay2')).toEqual(port('pci-1-ata-2'))
    expect(modelBinding(model, 'nope')).toBeNull()
    const swapped = overrideSlot(model, {}, 'bay1', port('pci-1-ata-2'))
    expect(swapped).toEqual({ bay1: port('pci-1-ata-2'), bay2: null })
    const back = overrideSlot(model, swapped, 'bay1', 'model')
    expect(back).toEqual({ bay2: null })
    expect(overrideSlot(model, {}, 'bay1', null)).toEqual({ bay1: null })
    const bySerial = overrideSlot(model, { bay2: serial('S') }, 'bay1', serial('S'))
    expect(bySerial).toEqual({ bay1: serial('S'), bay2: null })
  })

  it('says what keeps a layout from being saved', () => {
    const fine = [newGroup([])]
    expect(problemsOf(fine)).toEqual([])
    expect(problemsOf([])).toEqual(['groups'])
    const unnamed = [{ ...fine[0]!, name: '  ' }]
    expect(problemsOf(unnamed)).toEqual(['name'])
    const unlabelled = [{ ...fine[0]!, slots: [{ id: 'a', label: '', binding: null }] }]
    expect(problemsOf(unlabelled)).toEqual(['label'])
    const empty = [{ ...fine[0]!, slots: [] }]
    expect(problemsOf(empty)).toEqual(['size'])
    expect(problemsOf(Array.from({ length: 17 }, () => fine[0]!))).toEqual(['groups'])
  })

  it('becomes the request that saves it', () => {
    expect(requestOf({ source: 'model', model: 'acme-2', overrides: { bay1: null } })).toEqual({
      source: 'model',
      model: 'acme-2',
      overrides: [{ slot: 'bay1', binding: null }],
    })
    const group = { ...newGroup([]), name: ' Front ' }
    group.slots[0]!.label = ' A '
    const request = requestOf({ source: 'custom', groups: [group] })
    expect(request).toMatchObject({ source: 'custom' })
    expect(request.source === 'custom' && request.groups[0]!.name).toBe('Front')
    expect(request.source === 'custom' && request.groups[0]!.slots[0]!.label).toBe('A')
  })
})

describe('the countdown', () => {
  it('counts the seconds left and writes them as minutes', () => {
    expect(secondsLeft(1_000, 940_500)).toBe(60)
    expect(secondsLeft(1_000, 2_000_000)).toBe(0)
    expect(formatCountdown(245)).toBe('4:05')
    expect(formatCountdown(-3)).toBe('0:00')
  })
})
