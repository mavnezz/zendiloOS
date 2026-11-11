// What the Network page needs to offer bridges, bonds and VLANs: which
// interfaces may join one, free names, and the plan the server gives in
// words.
import type {
  InterfaceFate,
  NetworkDevice,
  NetworkInfo,
  NetworkInterface,
  PlannedSettings,
} from '@/shared/api/types'

export type DeviceKind = 'bridge' | 'bond' | 'vlan'

/** The longest interface name the kernel takes. */
export const NAME_MAX = 15

type Translate = (key: string, params?: Record<string, unknown>) => string

/** Whether a VLAN is on `name`: one zendiloOS made, or one the kernel shows. */
function carriesVlan(name: string, network: NetworkInfo, devices: NetworkDevice[]): boolean {
  return (
    network.interfaces.some((i) => i.parent === name) ||
    devices.some((d) => d.kind === 'vlan' && d.parent === name)
  )
}

/**
 * What may be a member of a bridge or bond (`device`, if it is there
 * already, keeps its own members): wired cards in no other bridge or bond
 * and carrying no VLAN; for a bridge also bonds and VLANs zendiloOS made.
 */
export function candidates(
  network: NetworkInfo,
  devices: NetworkDevice[],
  kind: 'bridge' | 'bond',
  device?: string,
): NetworkInterface[] {
  return network.interfaces.filter((item) => {
    if (item.name === device) return false
    if (item.member_of && item.member_of !== device) return false
    if (carriesVlan(item.name, network, devices)) return false
    if (item.kind === 'ethernet') return true
    const made = devices.find((d) => d.name === item.name)
    return kind === 'bridge' && (made?.kind === 'bond' || made?.kind === 'vlan')
  })
}

/** Where a VLAN may go: a wired card, a bond or a bridge in no other device. */
export function parents(network: NetworkInfo): NetworkInterface[] {
  return network.interfaces.filter(
    (item) => ['ethernet', 'bond', 'bridge'].includes(item.kind) && !item.member_of,
  )
}

/** The first of `prefix0`, `prefix1`, … no interface has. */
export function freeName(prefix: string, network: NetworkInfo, devices: NetworkDevice[]): string {
  const taken = new Set([...network.interfaces.map((i) => i.name), ...devices.map((d) => d.name)])
  let index = 0
  while (taken.has(`${prefix}${index}`)) index += 1
  return `${prefix}${index}`
}

/** A VLAN's usual name, `<parent>.<id>`, or `vlan<id>` where that is too long. */
export function vlanName(parent: string, id: number): string {
  const usual = `${parent}.${id}`
  return parent && usual.length <= NAME_MAX ? usual : `vlan${id}`
}

/** An interface's settings in a line: `DHCP, keeping 192.168.1.20 · IPv6 off`. */
export function describeSettings(settings: PlannedSettings, t: Translate): string {
  const parts: string[] = []
  const ipv4 = settings.ipv4
  if (ipv4.mode === 'dhcp') {
    parts.push(
      ipv4.keep
        ? t('network.takeover.dhcpKeeps', { address: ipv4.keep })
        : t('network.takeover.dhcp'),
    )
  } else if (ipv4.mode === 'static') {
    parts.push(`${ipv4.address}/${ipv4.prefix}`)
    if (ipv4.gateway) parts.push(t('network.takeover.via', { gateway: ipv4.gateway }))
  } else {
    parts.push(t('network.takeover.ipv4Off'))
  }
  parts.push(
    settings.ipv6 === 'automatic'
      ? t('network.takeover.ipv6Automatic')
      : t('network.takeover.ipv6Off'),
  )
  if (settings.dns.length) {
    parts.push(t('network.takeover.dns', { servers: settings.dns.join(', ') }))
  }
  return parts.join(' · ')
}

/** What becomes of an interface, in a sentence. */
export function describeFate(fate: InterfaceFate, t: Translate): string {
  const becomes = fate.becomes
  switch (becomes.fate) {
    case 'member':
      if (becomes.moved) return t('network.plan.moves', { device: becomes.device })
      if (becomes.dropped) return t('network.plan.drops', { device: becomes.device })
      return t('network.plan.member', { device: becomes.device })
    case 'settings': {
      const off = becomes.settings.ipv4.mode === 'off' && becomes.settings.ipv6 === 'off'
      return off ? t('network.plan.noAddress') : describeSettings(becomes.settings, t)
    }
    case 'switched_off':
      return t('network.plan.switchedOff')
    case 'parent':
      return becomes.carries
        ? t('network.plan.carries', { vlan: becomes.vlan })
        : t('network.plan.releases', { vlan: becomes.vlan })
  }
}

/** What a card on the page says the interface is made of, if anything. */
export function composition(
  item: NetworkInterface,
  network: NetworkInfo,
  devices: NetworkDevice[],
  t: Translate,
): string | null {
  const made = devices.find((d) => d.name === item.name)
  if (item.kind === 'vlan' || made?.kind === 'vlan') {
    const parent = made?.parent ?? item.parent
    if (!parent) return null
    return made?.vlan_id
      ? t('network.devices.onParentId', { parent, id: made.vlan_id })
      : t('network.devices.onParent', { parent })
  }
  if (item.kind !== 'bridge' && item.kind !== 'bond') return null
  const members = made?.members.length
    ? made.members
    : network.interfaces.filter((i) => i.member_of === item.name).map((i) => i.name)
  return members.length
    ? t('network.devices.members', { members: members.join(', ') })
    : t('network.devices.noMembers')
}
