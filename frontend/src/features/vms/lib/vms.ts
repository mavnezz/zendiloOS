// Virtual machines as the wizard and the list see them: what a preset
// fills in, what the machine allows, and how a state is shown. The server
// checks the same rules; these only spare a round trip.

import type {
  CreateVmRequest,
  VirtualMachine,
  VmFirmware,
  VmOs,
  VmPlugin,
  VmPowerState,
} from '@/shared/api/types'

export const NAME_MAX = 48
export const MEMORY_MIN_MIB = 128
export const DISK_MAX_GIB = 64 * 1024
const MIB = 1024 * 1024

export const OS_PRESETS: VmOs[] = ['linux', 'windows', 'other']

/** What each preset fills in; the same as the server's. */
export const PRESETS: Record<
  VmOs,
  { cpus: number; memory_mib: number; disk_gib: number; firmware: VmFirmware }
> = {
  linux: { cpus: 2, memory_mib: 2048, disk_gib: 20, firmware: 'uefi' },
  windows: { cpus: 2, memory_mib: 4096, disk_gib: 64, firmware: 'uefi' },
  other: { cpus: 1, memory_mib: 1024, disk_gib: 16, firmware: 'bios' },
}

/** An ISO picked in a share. */
export interface IsoPick {
  shareId: number
  /** In the share. */
  path: string
  /** `share/folder/file.iso`, as shown. */
  shown: string
}

export interface VmForm {
  name: string
  os: VmOs
  cpus: number
  memoryMib: number
  firmware: VmFirmware
  diskGib: number
  iso: IsoPick | null
  /** `null`: through the NAS (passt); otherwise the bridge. */
  bridge: string | null
  autostart: boolean
}

/** What the machine allows a VM: all its threads and all its memory. */
export function limits(plugin: VmPlugin | undefined): { cpus: number; memoryMib: number } {
  return {
    cpus: Math.max(1, Math.min(256, plugin?.cores ?? 1)),
    memoryMib: Math.floor((plugin?.memory_bytes ?? 0) / MIB),
  }
}

/** A preset's settings, held to what the machine has. */
export function presetForm(os: VmOs, plugin: VmPlugin | undefined, name = ''): VmForm {
  const preset = PRESETS[os]
  const most = limits(plugin)
  return {
    name,
    os,
    cpus: Math.min(preset.cpus, most.cpus),
    memoryMib: Math.max(MEMORY_MIN_MIB, Math.min(preset.memory_mib, most.memoryMib)),
    firmware: preset.firmware,
    diskGib: preset.disk_gib,
    iso: null,
    bridge: null,
    autostart: false,
  }
}

/** Another preset chosen: its settings, keeping the name, the ISO and the network. */
export function withPreset(form: VmForm, os: VmOs, plugin: VmPlugin | undefined): VmForm {
  return { ...presetForm(os, plugin, form.name), iso: form.iso, bridge: form.bridge }
}

export type FormProblem = 'name' | 'nameTaken' | 'cpus' | 'memory' | 'disk'

/** What breaks a rule, by the step it belongs to: names first, then resources, then the disk. */
export function problems(
  form: VmForm,
  plugin: VmPlugin | undefined,
  taken: string[],
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
  if (!Number.isInteger(form.diskGib) || form.diskGib < 1 || form.diskGib > DISK_MAX_GIB)
    found.push('disk')
  return found
}

/** The problems a wizard's step (from 0) must have none of before going on. */
export const STEP_PROBLEMS: FormProblem[][] = [
  ['name', 'nameTaken'],
  ['cpus', 'memory'],
  ['disk'],
  [],
]

/** What the server is asked for. */
export function request(form: VmForm): CreateVmRequest {
  return {
    name: form.name.trim(),
    os: form.os,
    cpus: form.cpus,
    memory_mib: form.memoryMib,
    firmware: form.firmware,
    disk_gib: form.diskGib,
    network: form.bridge ? { kind: 'bridge', bridge: form.bridge } : { kind: 'user' },
    autostart: form.autostart,
    iso: form.iso ? { share_id: form.iso.shareId, path: form.iso.path } : null,
  }
}

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

/** An ISO picked in the folder picker (`share/folder/file.iso`), with its share's id. */
export function isoPick(
  shown: string,
  shares: { id: number; name: string }[] | undefined,
): IsoPick | null {
  const [share = '', ...rest] = shown.split('/')
  const found = shares?.find((candidate) => candidate.name === share)
  const path = rest.join('/')
  return found && path ? { shareId: found.id, path, shown } : null
}
