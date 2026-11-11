// Virtual machines as the wizard, the edit dialog and the list see them:
// what a preset fills in, what the machine allows, which devices a VM
// has, what changes while it runs, and how a state is shown. The server
// checks the same rules; these only spare a round trip.

import type {
  CreateVmRequest,
  UpdateVmRequest,
  VirtualMachine,
  VmBootDevice,
  VmDiskBus,
  VmFirmware,
  VmImage,
  VmNetwork,
  VmNicModel,
  VmOs,
  VmPlugin,
  VmPowerState,
} from '@/shared/api/types'

export const NAME_MAX = 48
export const IMAGE_NAME_MAX = 128
export const MEMORY_MIN_MIB = 128
/** Memory is chosen in steps of this, as the wizard's field takes it. */
export const MEMORY_STEP_MIB = 128
export const DISK_MAX_GIB = 64 * 1024
export const DISKS_MAX = 8
export const NICS_MAX = 4
export const DRIVES_MAX = 3
/** What a disk added later has, unless changed. */
export const NEW_DISK_GIB = 32
const MIB = 1024 * 1024

export const OS_PRESETS: VmOs[] = ['linux', 'windows', 'other']
export const DISK_BUSES: VmDiskBus[] = ['virtio', 'sata']
export const NIC_MODELS: VmNicModel[] = ['virtio', 'e1000e']

/** What each preset fills in; the same as the server's. */
export const PRESETS: Record<
  VmOs,
  {
    cpus: number
    memory_mib: number
    disk_gib: number
    firmware: VmFirmware
    bus: VmDiskBus
    model: VmNicModel
  }
> = {
  linux: {
    cpus: 2,
    memory_mib: 2048,
    disk_gib: 20,
    firmware: 'uefi',
    bus: 'virtio',
    model: 'virtio',
  },
  windows: {
    cpus: 2,
    memory_mib: 4096,
    disk_gib: 64,
    firmware: 'uefi',
    bus: 'sata',
    model: 'e1000e',
  },
  other: {
    cpus: 1,
    memory_mib: 1024,
    disk_gib: 16,
    firmware: 'bios',
    bus: 'sata',
    model: 'e1000e',
  },
}

export interface DiskForm {
  slot: number
  sizeGib: number
  bus: VmDiskBus
  /** The pool of the location it lies on; `null`: the first, the library's. */
  pool: string | null
}

/** How a card reaches the network. */
export type NetworkKind = 'user' | 'bridge' | 'direct' | 'internal'
export const NETWORK_KINDS: NetworkKind[] = ['user', 'bridge', 'direct', 'internal']

export interface NicForm {
  slot: number
  model: VmNicModel
  kind: NetworkKind
  /** The bridge, card or internal network; `null` through the NAS, or none chosen yet. */
  device: string | null
}

export interface DriveForm {
  slot: number
  /** An image of the library, or empty. */
  image: string | null
}

export type BootEntry = VmBootDevice

export interface VmForm {
  name: string
  os: VmOs
  cpus: number
  memoryMib: number
  firmware: VmFirmware
  disks: DiskForm[]
  nics: NicForm[]
  drives: DriveForm[]
  /** The devices it boots from, first first. */
  boot: BootEntry[]
  autostart: boolean
}

/** What the machine allows a VM: all its threads and all its memory. */
export function limits(plugin: VmPlugin | undefined): { cpus: number; memoryMib: number } {
  return {
    cpus: Math.max(1, Math.min(256, plugin?.cores ?? 1)),
    memoryMib: Math.floor((plugin?.memory_bytes ?? 0) / MIB),
  }
}

/**
 * A preset's settings, held to what the machine has: one disk, one card
 * through the NAS and one empty drive, booted from the drive first. Of its
 * memory a VM is offered at most half, in whole steps: the rest stays for
 * the NAS and ZFS's cache, and a value off the field's steps could not be
 * sent.
 */
export function presetForm(os: VmOs, plugin: VmPlugin | undefined, name = ''): VmForm {
  const preset = PRESETS[os]
  const most = limits(plugin)
  const half = Math.floor(most.memoryMib / 2 / MEMORY_STEP_MIB) * MEMORY_STEP_MIB
  return {
    name,
    os,
    cpus: Math.min(preset.cpus, most.cpus),
    memoryMib: Math.max(MEMORY_MIN_MIB, Math.min(preset.memory_mib, half)),
    firmware: preset.firmware,
    disks: [{ slot: 0, sizeGib: preset.disk_gib, bus: preset.bus, pool: null }],
    nics: [{ slot: 0, model: preset.model, kind: 'user', device: null }],
    drives: [{ slot: 0, image: null }],
    boot: [
      { kind: 'drive', slot: 0 },
      { kind: 'disk', slot: 0 },
    ],
    autostart: false,
  }
}

/**
 * Another preset chosen: its resources, firmware, buses and card models,
 * keeping the name, the devices there are, their networks and images.
 */
export function withPreset(form: VmForm, os: VmOs, plugin: VmPlugin | undefined): VmForm {
  const fresh = presetForm(os, plugin, form.name)
  const preset = PRESETS[os]
  return {
    ...fresh,
    disks: form.disks.map((disk, index) => ({
      ...disk,
      sizeGib: index === 0 ? preset.disk_gib : disk.sizeGib,
      bus: preset.bus,
    })),
    nics: form.nics.map((nic) => ({ ...nic, model: preset.model })),
    drives: form.drives,
    boot: form.boot,
    autostart: form.autostart,
  }
}

/** The lowest slot below `max` none of `taken` has. */
function freeSlot(taken: { slot: number }[], max: number): number | null {
  for (let slot = 0; slot < max; slot++) if (!taken.some((item) => item.slot === slot)) return slot
  return null
}

const sameEntry = (a: BootEntry, b: BootEntry) => a.kind === b.kind && a.slot === b.slot

/** A disk more, of the preset's bus; it boots only if put in the boot order. */
export function addDisk(form: VmForm): VmForm {
  const slot = freeSlot(form.disks, DISKS_MAX)
  if (slot === null) return form
  const bus = form.disks[0]?.bus ?? PRESETS[form.os].bus
  return { ...form, disks: [...form.disks, { slot, sizeGib: NEW_DISK_GIB, bus, pool: null }] }
}

export function removeDisk(form: VmForm, slot: number): VmForm {
  return withBoot({
    ...form,
    disks: form.disks.filter((disk) => disk.slot !== slot),
    boot: form.boot.filter((entry) => !sameEntry(entry, { kind: 'disk', slot })),
  })
}

export function addNic(form: VmForm): VmForm {
  const slot = freeSlot(form.nics, NICS_MAX)
  if (slot === null) return form
  return {
    ...form,
    nics: [...form.nics, { slot, model: PRESETS[form.os].model, kind: 'user', device: null }],
  }
}

export function removeNic(form: VmForm, slot: number): VmForm {
  return { ...form, nics: form.nics.filter((nic) => nic.slot !== slot) }
}

/** A drive more, empty, booted after the others. */
export function addDrive(form: VmForm): VmForm {
  const slot = freeSlot(form.drives, DRIVES_MAX)
  if (slot === null) return form
  return { ...form, drives: [...form.drives, { slot, image: null }] }
}

export function removeDrive(form: VmForm, slot: number): VmForm {
  return withBoot({
    ...form,
    drives: form.drives.filter((drive) => drive.slot !== slot),
    boot: form.boot.filter((entry) => !sameEntry(entry, { kind: 'drive', slot })),
  })
}

/** A boot order that is never empty while there is a disk: the first one then. */
function withBoot(form: VmForm): VmForm {
  if (form.boot.length || !form.disks.length) return form
  return { ...form, boot: [{ kind: 'disk', slot: form.disks[0]!.slot }] }
}

/** Every device the VM may boot from, those in the boot order first, in it. */
export function bootChoices(form: VmForm): { entry: BootEntry; on: boolean }[] {
  const all: BootEntry[] = [
    ...form.disks.map((disk) => ({ kind: 'disk' as const, slot: disk.slot })),
    ...form.drives.map((drive) => ({ kind: 'drive' as const, slot: drive.slot })),
  ]
  const on = form.boot.filter((entry) => all.some((device) => sameEntry(device, entry)))
  const off = all.filter((device) => !on.some((entry) => sameEntry(device, entry)))
  return [
    ...on.map((entry) => ({ entry, on: true })),
    ...off.map((entry) => ({ entry, on: false })),
  ]
}

/** The device moved earlier (`-1`) or later (`1`) in the boot order. */
export function moveBoot(form: VmForm, entry: BootEntry, by: -1 | 1): VmForm {
  const boot = [...form.boot]
  const at = boot.findIndex((other) => sameEntry(other, entry))
  const to = at + by
  if (at < 0 || to < 0 || to >= boot.length) return form
  ;[boot[at], boot[to]] = [boot[to]!, boot[at]!]
  return { ...form, boot }
}

/** The device booted from last, or not at all. */
export function toggleBoot(form: VmForm, entry: BootEntry): VmForm {
  const on = form.boot.some((other) => sameEntry(other, entry))
  return {
    ...form,
    boot: on ? form.boot.filter((other) => !sameEntry(other, entry)) : [...form.boot, entry],
  }
}

export type FormProblem =
  'name' | 'nameTaken' | 'cpus' | 'memory' | 'disk' | 'diskShrunk' | 'boot' | 'image' | 'network'

/**
 * What a card of `kind` may go on: the machine's bridges, internal networks
 * or cards, and the one it is on (which may have gone since).
 */
export function networkChoices(
  plugin: VmPlugin | undefined,
  kind: NetworkKind,
  current: string | null = null,
): string[] {
  const offered =
    kind === 'bridge'
      ? (plugin?.bridges ?? [])
      : kind === 'direct'
        ? (plugin?.cards ?? [])
        : kind === 'internal'
          ? (plugin?.internal_networks ?? [])
          : []
  return current && kind !== 'user' && !offered.includes(current) ? [...offered, current] : offered
}

/** A card on another kind of network: on the first of its kind there is. */
export function withNetworkKind(
  nic: NicForm,
  kind: NetworkKind,
  plugin: VmPlugin | undefined,
): NicForm {
  if (kind === nic.kind) return nic
  return {
    ...nic,
    kind,
    device: kind === 'user' ? null : (networkChoices(plugin, kind)[0] ?? null),
  }
}

/**
 * What breaks a rule, by the step it belongs to: names first, then
 * resources, then the disks and media. Changing a VM (`before`), its disks
 * only grow.
 */
export function problems(
  form: VmForm,
  plugin: VmPlugin | undefined,
  taken: string[],
  images: VmImage[] = [],
  before?: VirtualMachine,
): FormProblem[] {
  const found: FormProblem[] = []
  const name = form.name.trim()
  if (!name || [...name].length > NAME_MAX || /[\u0000-\u001f\u007f]/.test(name)) found.push('name')
  else if (taken.some((other) => other.toLowerCase() === name.toLowerCase()))
    found.push('nameTaken')
  const most = limits(plugin)
  if (!Number.isInteger(form.cpus) || form.cpus < 1 || form.cpus > most.cpus) found.push('cpus')
  if (
    !Number.isInteger(form.memoryMib) ||
    form.memoryMib < MEMORY_MIN_MIB ||
    form.memoryMib > most.memoryMib
  )
    found.push('memory')
  if (
    !form.disks.length ||
    form.disks.some(
      (disk) => !Number.isInteger(disk.sizeGib) || disk.sizeGib < 1 || disk.sizeGib > DISK_MAX_GIB,
    )
  )
    found.push('disk')
  const had = (slot: number) => before?.disks.find((disk) => disk.slot === slot)?.size_gib
  if (form.disks.some((disk) => disk.sizeGib < (had(disk.slot) ?? 0))) found.push('diskShrunk')
  const has = (entry: BootEntry) =>
    entry.kind === 'disk'
      ? form.disks.some((disk) => disk.slot === entry.slot)
      : form.drives.some((drive) => drive.slot === entry.slot)
  if (!form.boot.length || !form.boot.every(has)) found.push('boot')
  const ready = new Set(images.filter((image) => image.ready).map((image) => image.id))
  if (form.drives.some((drive) => drive.image !== null && !ready.has(drive.image)))
    found.push('image')
  if (form.nics.some((nic) => nic.kind !== 'user' && !nic.device)) found.push('network')
  return found
}

/** The least a disk in `slot` may have: what it had, if it was there. */
export function minimumGib(before: VirtualMachine | undefined, slot: number): number {
  return before?.disks.find((disk) => disk.slot === slot)?.size_gib ?? 1
}

/** The problems a wizard's step (from 0) must have none of before going on. */
export const STEP_PROBLEMS: FormProblem[][] = [
  ['name', 'nameTaken'],
  ['cpus', 'memory'],
  ['disk', 'boot', 'image'],
  ['network'],
]

/** A card's network as the server takes it. */
export function network(nic: Pick<NicForm, 'kind' | 'device'>): VmNetwork {
  const device = nic.device ?? ''
  switch (nic.kind) {
    case 'bridge':
      return { kind: 'bridge', bridge: device }
    case 'direct':
      return { kind: 'direct', card: device }
    case 'internal':
      return { kind: 'internal', network: device }
    default:
      return { kind: 'user' }
  }
}

/** The kind and device of a card's network, as the form has them. */
export function fromNetwork(network: VmNetwork): Pick<NicForm, 'kind' | 'device'> {
  switch (network.kind) {
    case 'bridge':
      return { kind: 'bridge', device: network.bridge }
    case 'direct':
      return { kind: 'direct', device: network.card }
    case 'internal':
      return { kind: 'internal', device: network.network }
    default:
      return { kind: 'user', device: null }
  }
}

function devices(form: VmForm) {
  return {
    disks: form.disks.map((disk) => ({
      slot: disk.slot,
      size_gib: disk.sizeGib,
      bus: disk.bus,
      pool: disk.pool,
    })),
    nics: form.nics.map((nic) => ({ slot: nic.slot, model: nic.model, network: network(nic) })),
    drives: form.drives.map((drive) => ({ slot: drive.slot, image: drive.image })),
    boot: form.boot.map((entry) => ({ kind: entry.kind, slot: entry.slot })),
  }
}

/** What the server is asked for to make a VM. */
export function request(form: VmForm): CreateVmRequest {
  return {
    name: form.name.trim(),
    os: form.os,
    cpus: form.cpus,
    memory_mib: form.memoryMib,
    firmware: form.firmware,
    ...devices(form),
    autostart: form.autostart,
  }
}

/** What the server is asked for to change one; `confirm` its name, for a disk removed. */
export function updateRequest(form: VmForm, confirm?: string): UpdateVmRequest {
  return {
    name: form.name.trim(),
    cpus: form.cpus,
    memory_mib: form.memoryMib,
    firmware: form.firmware,
    ...devices(form),
    autostart: form.autostart,
    confirm: confirm ?? null,
  }
}

/** A VM as the edit dialog's form has it. */
export function fromVm(vm: VirtualMachine): VmForm {
  return {
    name: vm.name,
    os: vm.os,
    cpus: vm.cpus,
    memoryMib: vm.memory_mib,
    firmware: vm.firmware,
    disks: vm.disks.map((disk) => ({
      slot: disk.slot,
      sizeGib: disk.size_gib,
      bus: disk.bus,
      pool: disk.pool,
    })),
    nics: vm.nics.map((nic) => ({ slot: nic.slot, model: nic.model, ...fromNetwork(nic.network) })),
    drives: vm.drives.map((drive) => ({ slot: drive.slot, image: drive.image ?? null })),
    boot: vm.boot.map((entry) => ({ kind: entry.kind, slot: entry.slot })),
    autostart: vm.autostart,
  }
}

/**
 * Whether the form changes the VM's hardware: everything but its name,
 * autostart and the images in its drives, which change while it runs.
 */
export function hardwareChanged(form: VmForm, vm: VirtualMachine): boolean {
  const was = fromVm(vm)
  const slots = (drives: DriveForm[]) =>
    drives
      .map((drive) => drive.slot)
      .sort()
      .join()
  const sorted = <T extends { slot: number }>(items: T[]) =>
    JSON.stringify([...items].sort((a, b) => a.slot - b.slot))
  return (
    form.cpus !== was.cpus ||
    form.memoryMib !== was.memoryMib ||
    form.firmware !== was.firmware ||
    sorted(form.disks) !== sorted(was.disks) ||
    sorted(form.nics) !== sorted(was.nics) ||
    slots(form.drives) !== slots(was.drives) ||
    JSON.stringify(form.boot) !== JSON.stringify(was.boot)
  )
}

/** The disks the VM has and the form has no longer: destroyed on saving. */
export function removedDisks(form: VmForm, vm: VirtualMachine): number[] {
  return vm.disks
    .map((disk) => disk.slot)
    .filter((slot) => !form.disks.some((disk) => disk.slot === slot))
}

/** What a VM's card asks for beyond starting, shutting down and its console. */
export type CardAction = 'force_off' | 'restart' | 'edit' | 'autostart' | 'delete'

/** Whether QEMU runs, so the console answers and it can be shut down. */
export function isOn(state: VmPowerState): boolean {
  return ['running', 'paused', 'shutting_down', 'suspended'].includes(state)
}

/** Whether its state is about to change by itself: worth asking again soon. */
export function settling(vm: VirtualMachine): boolean {
  return vm.state === 'creating' || vm.state === 'shutting_down'
}

/** How a state is shown. */
export function stateVariant(
  state: VmPowerState,
): 'success' | 'secondary' | 'warning' | 'destructive' | 'info' {
  switch (state) {
    case 'running':
      return 'success'
    case 'crashed':
      return 'destructive'
    case 'creating':
    case 'shutting_down':
      return 'info'
    case 'paused':
    case 'suspended':
      return 'warning'
    default:
      return 'secondary'
  }
}

/** `256 MiB`, `2 GiB`, `1.5 GiB`. */
export function formatMemory(mib: number): string {
  if (mib < 1024) return `${mib} MiB`
  const gib = mib / 1024
  return `${Number.isInteger(gib) ? gib : gib.toFixed(1)} GiB`
}

/** The disks' sizes added up. */
export function totalDiskGib(vm: VirtualMachine): number {
  return vm.disks.reduce((total, disk) => total + disk.size_gib, 0)
}

/** An ISO picked in the folder picker (`share/folder/file.iso`), with its share's id. */
export function isoPick(
  shown: string,
  shares: { id: number; name: string }[] | undefined,
): { shareId: number; path: string; shown: string } | null {
  const [share = '', ...rest] = shown.split('/')
  const found = shares?.find((candidate) => candidate.name === share)
  const path = rest.join('/')
  return found && path ? { shareId: found.id, path, shown } : null
}

/** The last part of a path: what an image is called unless named otherwise. */
export function fileName(path: string): string {
  return path.split('/').filter(Boolean).at(-1) ?? ''
}

/** Whether an image's name is one the library takes. */
export function imageNameValid(name: string): boolean {
  const trimmed = name.trim()
  return (
    trimmed !== '' &&
    [...trimmed].length <= IMAGE_NAME_MAX &&
    !/[\u0000-\u001f\u007f]/.test(trimmed)
  )
}
