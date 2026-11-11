// The networks VMs can use, as their page lists them: through the NAS,
// bridges of the host, directly on a card, internal networks; each with
// the VMs whose cards are on it.

import type { VirtualMachine, VmPlugin } from '@/shared/api/types'

import { fromNetwork, type NetworkKind } from './vms'

export interface NetworkUser {
  id: string
  name: string
  /** The slots of its cards on it. */
  cards: number[]
}

export interface NetworkEntry {
  kind: NetworkKind
  /** The bridge, card or internal network; none through the NAS. */
  device: string | null
  /** The NAS has it; a card may stay on one gone since. */
  present: boolean
  vms: NetworkUser[]
}

/** The VMs whose cards are on `kind` and `device`. */
function users(vms: VirtualMachine[], kind: NetworkKind, device: string | null): NetworkUser[] {
  return vms
    .map((vm) => ({
      id: vm.id,
      name: vm.name,
      cards: vm.nics
        .filter((nic) => {
          const network = fromNetwork(nic.network)
          return network.kind === kind && network.device === device
        })
        .map((nic) => nic.slot),
    }))
    .filter((user) => user.cards.length > 0)
}

/** Every network of each kind, those the NAS has first, then those gone that a card is still on. */
export function vmNetworks(
  plugin: VmPlugin | undefined,
  vms: VirtualMachine[] = [],
): Record<NetworkKind, NetworkEntry[]> {
  const of = (kind: Exclude<NetworkKind, 'user'>, offered: string[]): NetworkEntry[] => {
    const used = vms.flatMap((vm) =>
      vm.nics.map((nic) => fromNetwork(nic.network)).filter((network) => network.kind === kind),
    )
    const gone = [...new Set(used.map((network) => network.device ?? ''))].filter(
      (device) => device && !offered.includes(device),
    )
    return [
      ...offered.map((device) => ({ kind, device, present: true, vms: users(vms, kind, device) })),
      ...gone
        .sort()
        .map((device) => ({ kind, device, present: false, vms: users(vms, kind, device) })),
    ]
  }
  return {
    user: [{ kind: 'user', device: null, present: true, vms: users(vms, 'user', null) }],
    bridge: of('bridge', plugin?.bridges ?? []),
    direct: of('direct', plugin?.cards ?? []),
    internal: of('internal', plugin?.internal_networks ?? []),
  }
}

/** An interface name the kernel takes: 1 to 15 letters, digits, `-`, `_` or `.`, not starting with `-` or `.`. */
export function interfaceNameValid(name: string): boolean {
  return /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,14}$/.test(name) && !name.startsWith('veth')
}

/** The first name `vmnet0`, `vmnet1`, … none of `taken` has. */
export function nextInternalName(taken: string[]): string {
  for (let n = 0; ; n++) if (!taken.includes(`vmnet${n}`)) return `vmnet${n}`
}
