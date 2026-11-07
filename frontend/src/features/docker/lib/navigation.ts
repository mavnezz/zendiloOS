import type { InjectionKey } from 'vue'

/** Opens a container's page in the Docker app, from wherever it is listed. */
export const OPEN_CONTAINER: InjectionKey<(id: string) => void> = Symbol('open container')

/** Where this browser remembers not to ask before updating a container. */
export const UPDATE_WITHOUT_ASKING = 'zendiloos.docker.updateWithoutAsking'

export function updatesWithoutAsking(): boolean {
  try {
    return window.localStorage.getItem(UPDATE_WITHOUT_ASKING) === 'true'
  } catch {
    return false
  }
}

export function updateWithoutAsking(): void {
  try {
    window.localStorage.setItem(UPDATE_WITHOUT_ASKING, 'true')
  } catch {
    // Without storage, the question comes again next time.
  }
}

/**
 * A host path below a volume as its volume and the share's folder:
 * `/volume1/media/Films` → volume 1, `media/Films`; `null` for any other.
 */
export function sharePath(path: string): { volume: number; path: string } | null {
  const match = /^\/volume(\d+)\/([^/@][^/]*(?:\/.*)?)$/.exec(path)
  return match ? { volume: Number(match[1]), path: match[2]! } : null
}
