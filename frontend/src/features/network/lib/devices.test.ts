import { describe, expect, it } from 'vitest'

import type { NetworkDevice, NetworkInfo, NetworkInterface } from '@/shared/api/types'

import {
  candidates,
  composition,
  describeFate,
  describeSettings,
  freeName,
  parents,
  vlanName,
} from './devices'

function iface(name: string, extra: Partial<NetworkInterface> = {}): NetworkInterface {
  return {
    name,
    kind: 'ethernet',
    state: 'up',
    mac: null,
    mtu: 1500,
    member_of: null,
    parent: null,
    addresses: [],
    ...extra,
  }
}

function device(name: string, extra: Partial<NetworkDevice> = {}): NetworkDevice {
  return {
    name,
    kind: 'bridge',
    members: [],
    parent: null,
    vlan_id: null,
    mode: null,
    stp: false,
    mac: null,
    ...extra,
  }
}

const network: NetworkInfo = {
  interfaces: [
    iface('enp1s0'),
    iface('enp2s0', { member_of: 'br0' }),
    iface('enp3s0'),
    iface('enp4s0'),
    iface('enp4s0.7', { kind: 'vlan', parent: 'enp4s0' }),
    iface('wlp1s0', { kind: 'wireless' }),
    iface('br0', { kind: 'bridge' }),
    iface('bond0', { kind: 'bond' }),
    iface('bond9', { kind: 'bond' }),
    iface('lab', { kind: 'vlan' }),
  ],
  default_routes: [],
  name_servers: [],
}
const devices = [
  device('br0', { members: ['enp2s0'] }),
  device('bond0', { kind: 'bond', members: [], mode: 'lacp' }),
  device('lab', { kind: 'vlan', parent: 'enp3s0', vlan_id: 30 }),
]
const names = (list: NetworkInterface[]) => list.map((i) => i.name)
const t = (key: string, params?: Record<string, unknown>) =>
  params ? `${key} ${JSON.stringify(params)}` : key

describe('devices', () => {
  it('offers free wired cards, and to a bridge the bonds and VLANs made here', () => {
    // enp2s0 is in br0, enp3s0 carries lab, enp4s0 carries enp4s0.7.
    expect(names(candidates(network, devices, 'bond'))).toEqual(['enp1s0'])
    expect(names(candidates(network, devices, 'bridge'))).toEqual(['enp1s0', 'bond0', 'lab'])
    // Its own members stay on offer, the device itself is none.
    expect(names(candidates(network, devices, 'bridge', 'br0'))).toEqual([
      'enp1s0',
      'enp2s0',
      'bond0',
      'lab',
    ])
    expect(names(parents(network))).toEqual(['enp1s0', 'enp3s0', 'enp4s0', 'br0', 'bond0', 'bond9'])
  })

  it('names new devices so they fit', () => {
    expect(freeName('br', network, devices)).toBe('br1')
    expect(freeName('vmbr', network, devices)).toBe('vmbr0')
    expect(vlanName('enp1s0', 10)).toBe('enp1s0.10')
    expect(vlanName('enx001122334455', 10)).toBe('vlan10')
    expect(vlanName('', 5)).toBe('vlan5')
  })

  it('says what becomes of each interface', () => {
    const dhcp = {
      ipv4: { mode: 'dhcp' as const, keep: '192.168.1.20' },
      ipv6: 'off' as const,
      dns: [],
    }
    expect(describeSettings(dhcp, t)).toBe(
      'network.takeover.dhcpKeeps {"address":"192.168.1.20"} · network.takeover.ipv6Off',
    )
    const fixed = {
      ipv4: { mode: 'static' as const, address: '10.0.0.5', prefix: 24, gateway: '10.0.0.1' },
      ipv6: 'automatic' as const,
      dns: ['9.9.9.9'],
    }
    expect(describeSettings(fixed, t)).toBe(
      '10.0.0.5/24 · network.takeover.via {"gateway":"10.0.0.1"} · network.takeover.ipv6Automatic · network.takeover.dns {"servers":"9.9.9.9"}',
    )
    expect(describeSettings({ ...dhcp, ipv4: { mode: 'dhcp', keep: null } }, t)).toContain(
      'network.takeover.dhcp ·',
    )
    const fate = (becomes: object) => describeFate({ interface: 'x', becomes } as never, t)
    expect(fate({ fate: 'member', device: 'br0', moved: true, dropped: false })).toBe(
      'network.plan.moves {"device":"br0"}',
    )
    expect(fate({ fate: 'member', device: 'br0', moved: false, dropped: true })).toBe(
      'network.plan.drops {"device":"br0"}',
    )
    expect(fate({ fate: 'member', device: 'br0', moved: false, dropped: false })).toBe(
      'network.plan.member {"device":"br0"}',
    )
    expect(fate({ fate: 'settings', settings: dhcp })).toContain('dhcpKeeps')
    const none = { ipv4: { mode: 'off' }, ipv6: 'off', dns: [] }
    expect(fate({ fate: 'settings', settings: none })).toBe('network.plan.noAddress')
    expect(fate({ fate: 'switched_off' })).toBe('network.plan.switchedOff')
    expect(fate({ fate: 'parent', vlan: 'v', carries: true })).toBe(
      'network.plan.carries {"vlan":"v"}',
    )
    expect(fate({ fate: 'parent', vlan: 'v', carries: false })).toBe(
      'network.plan.releases {"vlan":"v"}',
    )
  })

  it('names the members and parents a card shows', () => {
    const shown = (name: string) =>
      composition(
        iface(
          name,
          network.interfaces.find((i) => i.name === name),
        ),
        network,
        devices,
        t,
      )
    expect(shown('br0')).toBe('network.devices.members {"members":"enp2s0"}')
    // A bond made here without members, one made elsewhere by the kernel's word.
    expect(shown('bond0')).toBe('network.devices.noMembers')
    expect(shown('lab')).toBe('network.devices.onParentId {"parent":"enp3s0","id":30}')
    expect(shown('enp4s0.7')).toBe('network.devices.onParent {"parent":"enp4s0"}')
    expect(composition(iface('vlan9', { kind: 'vlan' }), network, devices, t)).toBeNull()
    expect(shown('enp1s0')).toBeNull()
    const bonded = {
      ...network,
      interfaces: [...network.interfaces, iface('enp5s0', { member_of: 'bond9' })],
    }
    expect(composition(iface('bond9', { kind: 'bond' }), bonded, devices, t)).toBe(
      'network.devices.members {"members":"enp5s0"}',
    )
  })
})
