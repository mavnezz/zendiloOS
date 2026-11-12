// The host's devices as the editor offers them to a VM: USB devices by
// vendor and product, PCI devices by IOMMU group, which go to a VM
// together. The server and the agent check the same rules again.

import type { VmHostPci, VmHostUsb, VmIommu } from '@/shared/api/types'

export const USB_MAX = 8
export const PCI_MAX = 8

/** A PCI-to-PCI bridge: it stays with the host, the rest of its group may go. */
export function isPciBridge(device: Pick<VmHostPci, 'class'>): boolean {
  return device.class.startsWith('0604')
}

/** The devices of one IOMMU group, as they go to a VM. */
export interface PciGroup {
  /** None for a device in no group, which cannot go at all. */
  id: number | null
  /** Every device of the group, bridges too. */
  devices: VmHostPci[]
  /** What goes to the VM: all but the bridges. */
  members: VmHostPci[]
  /** Why the group stays with the host: a device it needs, or no group. */
  blocked: VmHostPci | null
}

/** The host's PCI devices by IOMMU group; groups of bridges alone are left out. */
export function pciGroups(devices: readonly VmHostPci[]): PciGroup[] {
  const groups = new Map<string, VmHostPci[]>()
  for (const device of devices) {
    const key =
      device.iommu_group === null || device.iommu_group === undefined
        ? `none:${device.address}`
        : `group:${device.iommu_group}`
    groups.set(key, [...(groups.get(key) ?? []), device])
  }
  return [...groups.values()]
    .map((list) => {
      const members = list.filter((device) => !isPciBridge(device))
      const id = list[0]?.iommu_group ?? null
      const blocked = id === null ? (members[0] ?? null) : (members.find((d) => d.needed) ?? null)
      return { id, devices: list, members, blocked }
    })
    .filter((group) => group.members.length > 0)
    .sort((a, b) => (a.members[0]?.address ?? '').localeCompare(b.members[0]?.address ?? ''))
}

/** Whether the VM is given all, some or none of a group's devices. */
export function groupChosen(pci: readonly string[], group: PciGroup): 'all' | 'some' | 'none' {
  const given = group.members.filter((device) => pci.includes(device.address)).length
  return given === 0 ? 'none' : given === group.members.length ? 'all' : 'some'
}

/** The VM's PCI devices with a group's added (`on`) or taken away, in order. */
export function withGroup(pci: readonly string[], group: PciGroup, on: boolean): string[] {
  const addresses = group.members.map((device) => device.address)
  const rest = pci.filter((address) => !addresses.includes(address))
  return (on ? [...rest, ...addresses] : rest).sort()
}

/** What kind of device a PCI class is, for its label. */
export type PciKind = 'network' | 'display' | 'storage' | 'usb' | 'audio' | 'bridge' | 'other'

export function pciKind(className: string): PciKind {
  if (className.startsWith('0c03')) return 'usb'
  switch (className.slice(0, 2)) {
    case '01':
      return 'storage'
    case '02':
      return 'network'
    case '03':
      return 'display'
    case '04':
      return 'audio'
    case '06':
      return 'bridge'
    default:
      return 'other'
  }
}

/** A PCI device as it is shown: its names, else its IDs. */
export function pciLabel(device: VmHostPci): string {
  const name = [device.vendor_name, device.device_name].filter(Boolean).join(' ')
  return name || `${device.vendor}:${device.device}`
}

/** A USB device the VM may be given: plugged in, or given before and gone since. */
export interface UsbChoice {
  id: string
  /** Its name and maker, if it says. */
  label: string
  /** Where it is plugged in; none if it is not. */
  port: string | null
  /** More than one of the same vendor and product: libvirt cannot tell them apart. */
  twins: boolean
}

/** The USB devices plugged in, one entry each, and those given that are not. */
export function usbChoices(plugged: readonly VmHostUsb[], given: readonly string[]): UsbChoice[] {
  const choices: UsbChoice[] = []
  for (const device of plugged) {
    if (choices.some((choice) => choice.id === device.id)) continue
    const label = [device.manufacturer, device.name].filter(Boolean).join(' ')
    choices.push({
      id: device.id,
      label: label || device.id,
      port: device.port,
      twins: plugged.filter((other) => other.id === device.id).length > 1,
    })
  }
  for (const id of given) {
    if (!choices.some((choice) => choice.id === id))
      choices.push({ id, label: id, port: null, twins: false })
  }
  return choices
}

/** The VM's USB devices with `id` added (`on`) or taken away, in order. */
export function withUsb(usb: readonly string[], id: string, on: boolean): string[] {
  const rest = usb.filter((other) => other !== id)
  return (on ? [...rest, id] : rest).sort()
}

/** Which hint says how to switch the IOMMU on. */
export function iommuHint(iommu: Pick<VmIommu, 'cpu_vendor'> | undefined): 'intel' | 'amd' | 'any' {
  return iommu?.cpu_vendor === 'intel' ? 'intel' : iommu?.cpu_vendor === 'amd' ? 'amd' : 'any'
}
