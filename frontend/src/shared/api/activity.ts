/**
 * When the person last used the page. Requests the page makes on its own
 * meanwhile (polling, a window refreshing) say so, and do not keep an
 * unattended session alive; what the person does always does.
 */

/** Reads after this long without input count as the page's own. */
export const ATTENDED_MS = 2 * 60 * 1000

let lastInput = Date.now()
let watching = false

function noted(): void {
  lastInput = Date.now()
}

/** Starts watching input once; harmless to call again. */
export function watchActivity(target: Pick<Window, 'addEventListener'> = window): void {
  if (watching) return
  watching = true
  for (const kind of ['pointerdown', 'keydown', 'wheel', 'touchstart']) {
    target.addEventListener(kind, noted, { passive: true, capture: true })
  }
}

/** Whether nobody used the page for a while. */
export function unattended(now: number = Date.now()): boolean {
  return now - lastInput > ATTENDED_MS
}

/** For tests: input just now, or `at`. */
export function noteInput(at: number = Date.now()): void {
  lastInput = at
}
