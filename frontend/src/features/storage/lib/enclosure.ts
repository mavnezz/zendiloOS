import type {
  Disk,
  DiskHealth,
  Enclosure,
  EnclosureModel,
  EnclosureNumbering,
  EnclosureOrientation,
  NewGroup,
  SetEnclosure,
  SlotBinding,
  SlotForm,
  SmartReport,
} from '@/shared/api/types'

/** The backend's limits (storage/domain/enclosure.rs). */
export const MAX_GROUPS = 16
export const MAX_GROUP_SLOTS = 60
export const MAX_LABEL_CHARS = 32
export const MAX_NAME_CHARS = 64

/** How a slot's tile is coloured. */
export type SlotTone = 'good' | 'warning' | 'failing' | 'asleep' | 'unknown' | 'empty' | 'missing'

/** From the disk's SMART report where there is one, else the monitor's last verdict. */
export function slotTone(
  slot: { state: string; health?: DiskHealth | null },
  report?: SmartReport,
): SlotTone {
  if (slot.state === 'empty') return 'empty'
  if (slot.state === 'missing') return 'missing'
  if (report?.status === 'standby') return 'asleep'
  const health = report?.status === 'available' ? report.health : slot.health
  return health ?? 'unknown'
}

/** The slot a disk sits in, with its group's name. */
export function slotOf(
  enclosure: Enclosure | undefined,
  disk: string,
): { label: string; group: string } | null {
  for (const group of enclosure?.groups ?? []) {
    const slot = group.slots.find((candidate) => candidate.disk === disk)
    if (slot) return { label: slot.label, group: group.name }
  }
  return null
}

/** Row and column, from 0 at the top left, of the slot at `index`; as the backend places them. */
export function cell(
  index: number,
  rows: number,
  columns: number,
  numbering: EnclosureNumbering,
): { row: number; column: number } {
  const r = Math.max(rows, 1)
  const c = Math.max(columns, 1)
  switch (numbering) {
    case 'right_to_left':
      return { row: Math.floor(index / c), column: c - 1 - (index % c) }
    case 'top_to_bottom':
      return { row: index % r, column: Math.floor(index / r) }
    case 'bottom_to_top':
      return { row: r - 1 - (index % r), column: Math.floor(index / r) }
    default:
      return { row: Math.floor(index / c), column: index % c }
  }
}

/** What a disk is bound by: its port, or its serial number on USB or without a port. */
export function bindingFor(
  disk: Pick<Disk, 'transport' | 'serial' | 'id_path'>,
): SlotBinding | null {
  if (disk.id_path && disk.transport !== 'usb') return { kind: 'port', value: disk.id_path }
  if (disk.serial) return { kind: 'serial', value: disk.serial }
  return disk.id_path ? { kind: 'port', value: disk.id_path } : null
}

/** Whether two bindings name the same port or disk; `-ata-1` and `-ata-1.0` are one port. */
export function sameBinding(a: SlotBinding | null, b: SlotBinding | null): boolean {
  if (!a || !b || a.kind !== b.kind) return false
  if (a.kind === 'serial') return a.value === b.value
  return portKey(a.value) === portKey(b.value)
}

function portKey(path: string): string {
  return path.replace(/-ata-(\d+)\.0(?=-|$)/g, '-ata-$1')
}

/** The disk a binding names, among those present. */
export function boundDisk(binding: SlotBinding | null, disks: Disk[]): Disk | undefined {
  if (!binding) return undefined
  return disks.find((disk) =>
    binding.kind === 'serial'
      ? disk.serial === binding.value
      : disk.id_path != null && sameBinding(binding, { kind: 'port', value: disk.id_path }),
  )
}

export interface DraftSlot {
  id: string
  label: string
  binding: SlotBinding | null
}

export interface DraftGroup {
  id: string
  name: string
  rows: number
  columns: number
  orientation: EnclosureOrientation
  form: SlotForm
  numbering: EnclosureNumbering
  slots: DraftSlot[]
}

/** What the enclosure dialog edits. */
export type Draft =
  | { source: 'model'; model: string; overrides: Record<string, SlotBinding | null> }
  | { source: 'custom'; groups: DraftGroup[] }

/** A model's groups as a layout of one's own, its bindings kept. */
export function groupsOf(model: EnclosureModel): DraftGroup[] {
  return model.groups.map((group) => ({
    id: group.id,
    name: group.name,
    rows: group.rows,
    columns: group.columns,
    orientation: group.orientation,
    form: group.form,
    numbering: group.numbering,
    slots: group.slots.map((slot) => ({
      id: slot.id,
      label: slot.label,
      binding: slot.binding ?? null,
    })),
  }))
}

/** What is chosen now, to edit; a model of the catalog when none is, the detected one first. */
export function draftOf(enclosure: Enclosure, models: EnclosureModel[]): Draft {
  if (enclosure.source === 'custom') {
    return {
      source: 'custom',
      groups: enclosure.groups.map((group) => ({
        id: group.id,
        name: group.name,
        rows: group.rows,
        columns: group.columns,
        orientation: group.orientation,
        form: group.form,
        numbering: group.numbering,
        slots: group.slots.map((slot) => ({
          id: slot.id,
          label: slot.label,
          binding: slot.binding ?? null,
        })),
      })),
    }
  }
  if (enclosure.source === 'model' && enclosure.model) {
    const overrides: Record<string, SlotBinding | null> = {}
    for (const slot of enclosure.groups.flatMap((group) => group.slots)) {
      if (slot.overridden) overrides[slot.id] = slot.binding ?? null
    }
    return { source: 'model', model: enclosure.model.id, overrides }
  }
  const model = enclosure.detected ?? models[0]
  return model
    ? { source: 'model', model: model.id, overrides: {} }
    : { source: 'custom', groups: [newGroup([])] }
}

/** Every id the groups and their slots use. */
export function idsOf(groups: DraftGroup[]): string[] {
  return groups.flatMap((group) => [group.id, ...group.slots.map((slot) => slot.id)])
}

function freeId(prefix: string, taken: Set<string>): string {
  for (let n = 1; ; n += 1) {
    const id = `${prefix}${n}`
    if (!taken.has(id)) return id
  }
}

/** A new group of four upright 3.5" bays, with ids no other group uses. */
export function newGroup(groups: DraftGroup[], name = 'Bays'): DraftGroup {
  const taken = new Set(idsOf(groups))
  const id = freeId('group', taken)
  taken.add(id)
  const group: DraftGroup = {
    id,
    name,
    rows: 1,
    columns: 0,
    orientation: 'vertical',
    form: 'inch_3_5',
    numbering: 'left_to_right',
    slots: [],
  }
  return resize(group, 1, 4, taken)
}

/**
 * The group with another number of rows and columns (at most 60 slots):
 * slots stay in their order, new ones are numbered on, ids unique among `taken`.
 */
export function resize(
  group: DraftGroup,
  rows: number,
  columns: number,
  taken: Set<string> = new Set(),
): DraftGroup {
  const r = Math.min(Math.max(Math.trunc(rows) || 1, 1), MAX_GROUP_SLOTS)
  const c = Math.min(Math.max(Math.trunc(columns) || 1, 1), Math.floor(MAX_GROUP_SLOTS / r))
  const used = new Set([...taken, ...group.slots.map((slot) => slot.id)])
  const slots = group.slots.slice(0, r * c)
  while (slots.length < r * c) {
    const id = freeId(`${group.id}-`, used)
    used.add(id)
    slots.push({ id, label: String(slots.length + 1), binding: null })
  }
  return { ...group, rows: r, columns: c, slots }
}

/** `slots` with `binding` at `slot`, taken away from any other slot that had it. */
export function bindSlot(
  groups: DraftGroup[],
  slot: string,
  binding: SlotBinding | null,
): DraftGroup[] {
  return groups.map((group) => ({
    ...group,
    slots: group.slots.map((candidate) => {
      if (candidate.id === slot) return { ...candidate, binding }
      return binding && sameBinding(candidate.binding, binding)
        ? { ...candidate, binding: null }
        : candidate
    }),
  }))
}

/** The overrides with `binding` at `slot` of `model`, taken away from other slots that had it. */
export function overrideSlot(
  model: EnclosureModel,
  overrides: Record<string, SlotBinding | null>,
  slot: string,
  binding: SlotBinding | null | 'model',
): Record<string, SlotBinding | null> {
  const next = { ...overrides }
  if (binding === 'model') delete next[slot]
  else next[slot] = binding
  const chosen = binding === 'model' ? modelBinding(model, slot) : binding
  if (!chosen) return next
  for (const other of model.groups.flatMap((group) => group.slots)) {
    if (other.id === slot) continue
    const current = other.id in next ? (next[other.id] ?? null) : (other.binding ?? null)
    if (sameBinding(current, chosen)) next[other.id] = null
  }
  return next
}

/** What the model itself binds at `slot`. */
export function modelBinding(model: EnclosureModel, slot: string): SlotBinding | null {
  for (const group of model.groups) {
    const found = group.slots.find((candidate) => candidate.id === slot)
    if (found) return found.binding ?? null
  }
  return null
}

/** What is wrong with a layout of one's own, as message keys; empty: it may be saved. */
export function problemsOf(groups: DraftGroup[]): string[] {
  const problems = new Set<string>()
  if (groups.length === 0 || groups.length > MAX_GROUPS) problems.add('groups')
  for (const group of groups) {
    const name = group.name.trim()
    if (!name || name.length > MAX_NAME_CHARS) problems.add('name')
    if (group.rows * group.columns > MAX_GROUP_SLOTS || group.slots.length === 0) {
      problems.add('size')
    }
    for (const slot of group.slots) {
      const label = slot.label.trim()
      if (!label || label.length > MAX_LABEL_CHARS) problems.add('label')
    }
  }
  return [...problems]
}

/** The request that saves the draft; names and labels trimmed. */
export function requestOf(draft: Draft): SetEnclosure {
  if (draft.source === 'model') {
    return {
      source: 'model',
      model: draft.model,
      overrides: Object.entries(draft.overrides).map(([slot, binding]) => ({ slot, binding })),
    }
  }
  return {
    source: 'custom',
    groups: draft.groups.map((group): NewGroup => ({
      id: group.id,
      name: group.name.trim(),
      rows: group.rows,
      columns: group.columns,
      orientation: group.orientation,
      form: group.form,
      numbering: group.numbering,
      slots: group.slots.map((slot) => ({
        id: slot.id,
        label: slot.label.trim(),
        binding: slot.binding,
      })),
    })),
  }
}

/** Seconds left until `until` (Unix seconds), never below zero. */
export function secondsLeft(until: number, nowMs: number): number {
  return Math.max(0, Math.ceil(until - nowMs / 1000))
}

/** A countdown such as `4:05`. */
export function formatCountdown(seconds: number): string {
  const whole = Math.max(0, Math.round(seconds))
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`
}
