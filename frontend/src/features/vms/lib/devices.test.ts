import { describe, expect, it } from 'vitest'

import type { VmHostPci, VmHostUsb } from '@/shared/api/types'

import {
  groupChosen,
  iommuHint,
  isPciBridge,
  pciGroups,
  pciKind,
  pciLabel,
  usbChoices,
  withGroup,
  withUsb,
} from './devices'

function pci(
  address: string,
  cls: string,
  group: number | null,
  extra: Partial<VmHostPci> = {},
): VmHostPci {
  return {
    address,
    vendor: '10de',
    device: '1c82',
    class: cls,
    vendor_name: 'NVIDIA Corporation',
    device_name: 'GP107',
    iommu_group: group,
    driver: null,
    needed: null,
    ...extra,
  }
}

const DEVICES: VmHostPci[] = [
  pci('0000:00:01.0', '060400', 1, { needed: { kind: 'bridge' } }),
  pci('0000:01:00.1', '040300', 1),
  pci('0000:01:00.0', '030000', 1),
  pci('0000:00:17.0', '010601', 2, { needed: { kind: 'disk', disk: 'sda' } }),
  pci('0000:00:00.0', '060000', 0, { needed: { kind: 'bridge' } }),
  pci('0000:05:00.0', '020000', null),
  pci('0000:00:1c.0', '060400', 7, { needed: { kind: 'bridge' } }),
]

describe('the host’s PCI devices', () => {
  it('go by IOMMU group, the bridges staying with the host', () => {
    const groups = pciGroups(DEVICES)
    expect(groups.map((group) => [group.id, group.members.map((d) => d.address)])).toEqual([
      [0, ['0000:00:00.0']],
      [2, ['0000:00:17.0']],
      [1, ['0000:01:00.1', '0000:01:00.0']],
      [null, ['0000:05:00.0']],
    ])
    const gpu = groups.find((group) => group.id === 1)!
    expect(gpu.devices).toHaveLength(3)
    expect(gpu.blocked).toBeNull()
    expect(groups.find((group) => group.id === 2)!.blocked?.address).toBe('0000:00:17.0')
    expect(groups.find((group) => group.id === 0)!.blocked?.address).toBe('0000:00:00.0')
    expect(groups.find((group) => group.id === null)!.blocked?.address).toBe('0000:05:00.0')
    expect(isPciBridge(DEVICES[0]!)).toBe(true)
  })

  it('are given and taken whole, in order', () => {
    const gpu = pciGroups(DEVICES).find((group) => group.id === 1)!
    expect(groupChosen([], gpu)).toBe('none')
    expect(groupChosen(['0000:01:00.0'], gpu)).toBe('some')
    const given = withGroup(['0000:09:00.0'], gpu, true)
    expect(given).toEqual(['0000:01:00.0', '0000:01:00.1', '0000:09:00.0'])
    expect(groupChosen(given, gpu)).toBe('all')
    expect(withGroup(given, gpu, false)).toEqual(['0000:09:00.0'])
  })

  it('are named by kind and name, else by their IDs', () => {
    expect(
      ['020000', '030000', '010601', '0c0330', '040300', '060400', '0c0500'].map(pciKind),
    ).toEqual(['network', 'display', 'storage', 'usb', 'audio', 'bridge', 'other'])
    expect(pciLabel(DEVICES[2]!)).toBe('NVIDIA Corporation GP107')
    expect(
      pciLabel(pci('0000:02:00.0', '020000', 3, { vendor_name: null, device_name: null })),
    ).toBe('10de:1c82')
  })

  it('say how to switch the IOMMU on for the CPU’s maker', () => {
    expect(iommuHint({ cpu_vendor: 'intel' })).toBe('intel')
    expect(iommuHint({ cpu_vendor: 'amd' })).toBe('amd')
    expect(iommuHint({ cpu_vendor: null })).toBe('any')
    expect(iommuHint(undefined)).toBe('any')
  })
})

describe('the host’s USB devices', () => {
  const plugged: VmHostUsb[] = [
    { id: '046d:c31c', port: '1-2', manufacturer: 'Logitech', name: 'USB Keyboard' },
    { id: '0781:5583', port: '1-3', manufacturer: null, name: null },
    { id: '0781:5583', port: '1-4', manufacturer: null, name: null },
  ]

  it('are offered once each, twins marked, and one given but unplugged kept', () => {
    expect(usbChoices(plugged, ['1234:5678'])).toEqual([
      { id: '046d:c31c', label: 'Logitech USB Keyboard', port: '1-2', twins: false },
      { id: '0781:5583', label: '0781:5583', port: '1-3', twins: true },
      { id: '1234:5678', label: '1234:5678', port: null, twins: false },
    ])
  })

  it('are given and taken, in order', () => {
    expect(withUsb(['0781:5583'], '046d:c31c', true)).toEqual(['046d:c31c', '0781:5583'])
    expect(withUsb(['046d:c31c', '0781:5583'], '046d:c31c', false)).toEqual(['0781:5583'])
  })
})
