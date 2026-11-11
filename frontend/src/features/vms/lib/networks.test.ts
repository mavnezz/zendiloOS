import { describe, expect, it } from 'vitest'

import type { VirtualMachine, VmPlugin } from '@/shared/api/types'

import { interfaceNameValid, nextInternalName, vmNetworks } from './networks'

const PLUGIN = {
  bridges: ['br0'],
  internal_networks: ['vmnet0'],
  cards: ['enp1s0', 'bond0'],
} as unknown as VmPlugin

function vm(
  id: string,
  name: string,
  networks: VirtualMachine['nics'][number]['network'][],
): VirtualMachine {
  return {
    id,
    name,
    nics: networks.map((network, slot) => ({
      slot,
      model: 'virtio',
      network,
      mac: `52:54:00:00:00:0${slot}`,
    })),
  } as unknown as VirtualMachine
}

describe('the networks of VMs', () => {
  it('list each kind with the VMs on each, and what a card is still on though gone', () => {
    const vms = [
      vm('000000000001', 'web', [{ kind: 'user' }, { kind: 'bridge', bridge: 'br0' }]),
      vm('000000000002', 'db', [
        { kind: 'direct', card: 'bond0' },
        { kind: 'internal', network: 'vmnet0' },
      ]),
      vm('000000000003', 'lab', [
        { kind: 'internal', network: 'vmnet0' },
        { kind: 'internal', network: 'old0' },
      ]),
    ]
    const networks = vmNetworks(PLUGIN, vms)
    expect(networks.user).toEqual([
      {
        kind: 'user',
        device: null,
        present: true,
        vms: [{ id: '000000000001', name: 'web', cards: [0] }],
      },
    ])
    expect(
      networks.bridge.map((entry) => [entry.device, entry.vms.map((user) => user.name)]),
    ).toEqual([['br0', ['web']]])
    expect(networks.direct.map((entry) => [entry.device, entry.vms.length])).toEqual([
      ['enp1s0', 0],
      ['bond0', 1],
    ])
    expect(networks.internal).toEqual([
      {
        kind: 'internal',
        device: 'vmnet0',
        present: true,
        vms: [
          { id: '000000000002', name: 'db', cards: [1] },
          { id: '000000000003', name: 'lab', cards: [0] },
        ],
      },
      {
        kind: 'internal',
        device: 'old0',
        present: false,
        vms: [{ id: '000000000003', name: 'lab', cards: [1] }],
      },
    ])
    expect(vmNetworks(undefined).bridge).toEqual([])
  })

  it('name a new internal network as the kernel takes it', () => {
    expect(nextInternalName([])).toBe('vmnet0')
    expect(nextInternalName(['vmnet0', 'vmnet1', 'br0'])).toBe('vmnet2')
    for (const good of ['vmnet0', 'lab_1', 'a', 'x'.repeat(15)])
      expect(interfaceNameValid(good), good).toBe(true)
    for (const bad of ['', '-lab', '.lab', 'x'.repeat(16), 'a b', 'veth1', 'ä'])
      expect(interfaceNameValid(bad), bad).toBe(false)
  })
})
