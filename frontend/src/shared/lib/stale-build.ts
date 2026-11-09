/**
 * A tab left open across an update of zendiloOS still runs the build it
 * loaded, whose lazily loaded parts are gone from the server: loading one
 * fails, and Vite says so with the event `vite:preloadError`. The page is
 * then reloaded once, which brings the new build. Should the parts of the
 * same build fail again soon after (the server not back yet, a page
 * cached), it is not reloaded on and on: a note asks to reload by hand.
 */
import { ref } from 'vue'

/** Where the tab notes that it reloaded, and for which build. */
export const RELOADED_KEY = 'zendiloos.reloadedForBuild'
/** A reload for a build counts as tried for this long. */
export const AGAIN_AFTER_MS = 5 * 60_000

/** Set once reloading did not help: the page shows a note. */
export const staleBuild = ref(false)

export interface Reload {
  /** What the tab loaded, e.g. the URL of its entry script: another after an update. */
  build: string
  /** Where the reload is noted (`sessionStorage`); none if the browser keeps nothing. */
  storage: Pick<Storage, 'getItem' | 'setItem'> | undefined
  now: () => number
  reload: () => void
}

/**
 * Reloads the page unless it reloaded for this build a moment ago; returns
 * whether it does. Without a place to note it, it never reloads: it could
 * not tell the second time from the first.
 */
export function reloadForNewBuild(how: Reload): boolean {
  let last: { build?: unknown; at?: unknown } = {}
  try {
    last = JSON.parse(how.storage?.getItem(RELOADED_KEY) ?? '{}') as typeof last
  } catch {
    last = {}
  }
  const tried =
    last.build === how.build && typeof last.at === 'number' && how.now() - last.at < AGAIN_AFTER_MS
  if (tried || !how.storage) return false
  try {
    how.storage.setItem(RELOADED_KEY, JSON.stringify({ build: how.build, at: how.now() }))
  } catch {
    return false
  }
  how.reload()
  return true
}

/**
 * Handles `vite:preloadError` on `target` (the window): the page reloads
 * for the new build, and Vite throws nothing meanwhile; when it reloaded
 * already, the error goes on as it came, and the note is shown.
 */
export function reloadOnStaleBuild(target: EventTarget, how: () => Reload): void {
  target.addEventListener('vite:preloadError', (event) => {
    if (reloadForNewBuild(how())) event.preventDefault()
    else staleBuild.value = true
  })
}
