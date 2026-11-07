import { computed, ref } from 'vue'

/** The personal folder's ID in the file routes. */
export const HOME = 0
/** Where deleted entries go in a folder on a volume: `.recycle/<user>`. */
export const RECYCLE_BIN = '.recycle'

/**
 * Where files are: a folder on a volume by its share ID (the personal
 * folder as {@link HOME}), or a removable drive by its ID, which the file
 * routes of `/files/removable/{drive}` take.
 */
export type Place = number | string

/** Whether `place` is a removable drive. */
export function isDrive(place: Place): place is string {
  return typeof place === 'string'
}

/** Where the explorer is. */
export type Location =
  /** The folders directly on a volume. */
  | { kind: 'volume'; volume: number }
  /** A folder in a folder on a volume or on a drive (`share`), or in the personal one. */
  | { kind: 'folder'; share: Place; path: string; snapshot: string | null }
  /** The signed-in person's recycle bins, of every folder, together. */
  | { kind: 'bin' }

export function folderAt(share: Place, path = '', snapshot: string | null = null): Location {
  return { kind: 'folder', share, path, snapshot }
}

export function sameLocation(a: Location, b: Location): boolean {
  if (a.kind === 'volume' && b.kind === 'volume') return a.volume === b.volume
  if (a.kind === 'folder' && b.kind === 'folder') {
    return a.share === b.share && a.path === b.path && a.snapshot === b.snapshot
  }
  return a.kind === b.kind
}

/** `parent/name`, or `name` at the top. */
export function joinPath(parent: string, name: string): string {
  return parent ? `${parent}/${name}` : name
}

/** The folder above `path`: `a/b` → `a`, `a` → the top. */
export function parentPath(path: string): string {
  return path.split('/').slice(0, -1).join('/')
}

/** The person's own bin in a folder, if they are known. */
export function ownBin(user: string | undefined): string | null {
  return user ? `${RECYCLE_BIN}/${user}` : null
}

export function inBin(path: string): boolean {
  return path.split('/')[0] === RECYCLE_BIN
}

/** Back and forward, as a browser has them. */
export function useHistory(start: Location) {
  const entries = ref<Location[]>([start])
  const at = ref(0)
  const current = computed(() => entries.value[at.value]!)
  return {
    current,
    canBack: computed(() => at.value > 0),
    canForward: computed(() => at.value < entries.value.length - 1),
    go(location: Location): void {
      if (sameLocation(location, current.value)) return
      entries.value = [...entries.value.slice(0, at.value + 1), location]
      at.value = entries.value.length - 1
    },
    /** Puts `location` in place of the current one, without a step back. */
    replace(location: Location): void {
      entries.value = entries.value.map((entry, index) => (index === at.value ? location : entry))
    },
    back(): void {
      if (at.value > 0) at.value -= 1
    },
    forward(): void {
      if (at.value < entries.value.length - 1) at.value += 1
    },
  }
}
