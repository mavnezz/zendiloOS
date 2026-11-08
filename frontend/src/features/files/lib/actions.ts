import type { FileKind, RemovableState } from '@/shared/api/types'

import type { Place } from './location'

/** Everything the context menu offers, in its order. */
export const ACTIONS = [
  'open',
  'openLocation',
  'download',
  'newFolder',
  'upload',
  'cut',
  'copy',
  'paste',
  'rename',
  'delete',
  'restore',
  'emptyBin',
  'copyBack',
  'favourite',
  'share',
  'previousVersions',
  'eject',
  'mount',
  'properties',
] as const

export type ActionId = (typeof ACTIONS)[number]

/** An entry as the explorer shows it, wherever it lives. */
export interface Item {
  /** Unique in the view. */
  key: string
  name: string
  kind: FileKind
  /** Bytes, for files. */
  size: number | null
  /** Unix seconds. */
  modified: number | null
  /** The name is not UTF-8: shown, never acted on. */
  unreadable: boolean
  /** The folder on a volume (share ID, `0` personal) or the drive it lives in. */
  share: Place
  /** In that folder; empty for the folder on the volume itself. */
  path: string
  /** For a folder directly on a volume. */
  volumeFolder?: {
    shared: boolean
    /** `null`: listed for someone who manages shares, but not to open. */
    access: 'read' | 'read_write' | null
  }
  /** For a removable drive itself, in the tree. */
  drive?: {
    state: RemovableState
    /** Why it is not mounted, if it could not be. */
    problem: string | null
  }
  /** In the recycle bin view the folder it was deleted from; found by a search, the folder it is in. */
  origin?: string
}

/** What the properties of an entry, or of the folder shown, tell. */
export interface EntryInfo {
  name: string
  type: string
  location: string
  size: number | null
  modified: number | null
  /** For a folder on a volume: what the person may do in it (`null`: nothing). */
  access?: 'read' | 'read_write' | null
  /** For a folder on a volume: whether it is on the network. */
  shared?: boolean
  /** Where to ask the NAS for its times, owner and (for a folder) what it holds. */
  at?: { place: Place; path: string; snapshot: string | null }
  folder?: boolean
}

/** What the explorer is looking at, and what is chosen there. */
export interface Situation {
  kind: 'volume' | 'folder' | 'bin'
  /** What is shown is what a search below the folder found. */
  searching: boolean
  /** Entries can be made, changed and removed here. */
  writable: boolean
  /** Looking into a snapshot: read only, entries can be copied back. */
  snapshot: boolean
  /** In the person's own recycle bin of a folder (where entries go back from). */
  ownBin: boolean
  /** At the top of the person's own bin, with entries in it. */
  binToEmpty: boolean
  /** The folder on a volume here may be written to (to copy back into). */
  shareWritable: boolean
  /** May create, change and delete folders on a volume (`shares.manage`). */
  canManage: boolean
  /** Something was cut or copied. */
  clipboard: boolean
  /** The folder the favourite action is about is one already. */
  favourite: boolean
  /** The favourite action has a folder to be about. */
  favouriteTarget: boolean
  /** Snapshots can be looked into here (a folder on a volume). */
  hasSnapshots: boolean
  /** The folder on a volume the share action is about, if any. */
  shareTarget: { shared: boolean } | null
  /** On a removable drive, or about one in the tree: its state. */
  drive: RemovableState | null
  /** The chosen entries; never unreadable ones. */
  items: Item[]
}

export interface ActionState {
  id: ActionId
  disabled: boolean
  /** The favourite action removes; the share action stops sharing. */
  undo?: boolean
}

function relevant(id: ActionId, s: Situation): boolean {
  switch (id) {
    case 'openLocation':
      return s.searching
    case 'restore':
    case 'emptyBin':
      return s.kind === 'bin' || s.ownBin
    case 'copyBack':
      return s.snapshot
    case 'share':
      return s.canManage && s.shareTarget !== null
    case 'upload':
    case 'cut':
    case 'paste':
    case 'rename':
      return s.kind === 'folder' && !s.snapshot
    case 'newFolder':
      return (s.kind === 'folder' && !s.snapshot) || (s.kind === 'volume' && s.canManage)
    case 'copy':
      return s.kind !== 'bin' && !s.snapshot
    case 'favourite':
      // Drives come and go: no favourites on them, no snapshots either.
      return s.kind !== 'bin' && !s.snapshot && s.drive === null
    case 'previousVersions':
      return s.kind !== 'bin' && s.drive === null
    case 'eject':
      return s.drive !== null
    case 'mount':
      return s.drive !== null && s.drive !== 'mounted'
    default:
      return true
  }
}

function enabled(id: ActionId, s: Situation): boolean {
  const one = s.items.length === 1 ? s.items[0] : undefined
  const some = s.items.length > 0
  const volumeFolders = s.items.some((item) => item.volumeFolder !== undefined)
  // A drive itself, in the tree: opened while mounted, otherwise only ejected or mounted.
  const drives = s.items.some((item) => item.drive !== undefined)
  switch (id) {
    case 'openLocation':
      return one !== undefined
    case 'open':
      return (
        one !== undefined &&
        one.volumeFolder?.access !== null &&
        (one.drive?.state ?? 'mounted') === 'mounted'
      )
    case 'download':
      return (
        one !== undefined &&
        (one.kind === 'file' || one.kind === 'directory') &&
        one.volumeFolder?.access !== null &&
        !drives
      )
    case 'newFolder':
      return s.kind === 'volume' ? s.canManage : s.writable
    case 'upload':
    case 'paste':
      return s.writable && (id === 'upload' || s.clipboard)
    case 'cut':
    case 'rename':
      return s.writable && some && !volumeFolders && !drives && (id === 'cut' || one !== undefined)
    case 'copy':
      return some && !volumeFolders && !drives
    case 'delete':
      if (drives) return false
      // A folder on a volume goes with its dataset: one at a time.
      if (s.kind === 'volume' || volumeFolders) return s.canManage && one !== undefined
      return some && (s.writable || s.kind === 'bin')
    case 'restore':
      return some && (s.kind === 'bin' || s.writable)
    case 'emptyBin':
      return s.binToEmpty
    case 'copyBack':
      return some && s.shareWritable
    case 'favourite':
      return s.favouriteTarget
    case 'share':
      return s.shareTarget !== null
    case 'previousVersions':
      return s.hasSnapshots
    case 'eject':
      return s.drive === 'mounted'
    case 'mount':
      return true
    case 'properties':
      return (one !== undefined || s.kind === 'folder') && (s.drive ?? 'mounted') === 'mounted'
  }
}

/** The actions that belong where the explorer is, each enabled or not. */
export function actionsFor(s: Situation): ActionState[] {
  return ACTIONS.filter((id) => relevant(id, s)).map((id) => ({
    id,
    disabled: !enabled(id, s),
    undo: (id === 'favourite' && s.favourite) || (id === 'share' && s.shareTarget?.shared === true),
  }))
}

/** Keys that run an action where the explorer has the focus. */
export function shortcut(
  event: KeyboardEvent,
): ActionId | 'selectAll' | 'up' | 'back' | 'forward' | null {
  const ctrl = event.ctrlKey || event.metaKey
  if (event.altKey && event.key === 'ArrowLeft') return 'back'
  if (event.altKey && event.key === 'ArrowRight') return 'forward'
  if ((event.altKey && event.key === 'ArrowUp') || event.key === 'Backspace') return 'up'
  if (event.key === 'Delete') return 'delete'
  if (event.key === 'F2') return 'rename'
  if (event.key === 'Enter') return 'open'
  if (!ctrl) return null
  switch (event.key.toLowerCase()) {
    case 'a':
      return 'selectAll'
    case 'c':
      return 'copy'
    case 'x':
      return 'cut'
    case 'v':
      return 'paste'
    default:
      return null
  }
}

/**
 * What dropping entries from `from` onto a folder of `into` does: within one
 * place (a folder on a volume, the personal folder, a drive) they move,
 * into another place they are copied, so nothing leaves a place by a slip
 * of the mouse. Ctrl (or Alt) copies and Shift moves, either way.
 */
export function dropMode(
  from: Place,
  into: Place,
  keys: Pick<MouseEvent, 'ctrlKey' | 'altKey' | 'shiftKey'>,
): 'copy' | 'move' {
  if (keys.ctrlKey || keys.altKey) return 'copy'
  if (keys.shiftKey) return 'move'
  return from === into ? 'move' : 'copy'
}

/** What a file is, by its extension: `PDF`, or `null` without one. */
export function extension(name: string): string | null {
  const dot = name.lastIndexOf('.')
  return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1).toUpperCase() : null
}

export type SortKey = 'name' | 'modified' | 'type' | 'size'

/** Folders first, then by `key`, names by their numbers too. */
export function sortItems(items: Item[], key: SortKey, descending: boolean): Item[] {
  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' })
  const by = (a: Item, b: Item): number => {
    switch (key) {
      case 'modified':
        return (a.modified ?? 0) - (b.modified ?? 0)
      case 'size':
        return (a.size ?? -1) - (b.size ?? -1)
      case 'type':
        return collator.compare(extension(a.name) ?? '', extension(b.name) ?? '')
      case 'name':
        return 0
    }
  }
  return [...items].sort((a, b) => {
    const folders = Number(b.kind === 'directory') - Number(a.kind === 'directory')
    if (folders) return folders
    const order = by(a, b) || collator.compare(a.name, b.name)
    return descending ? -order : order
  })
}
