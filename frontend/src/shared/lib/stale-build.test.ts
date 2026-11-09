import { beforeEach, describe, expect, it, vi } from 'vitest'

import {
  AGAIN_AFTER_MS,
  RELOADED_KEY,
  type Reload,
  reloadForNewBuild,
  reloadOnStaleBuild,
  staleBuild,
} from './stale-build'

/** A tab of `build` at `now`, its session kept in `kept`. */
function tab(kept: Map<string, string>, build: string, now: number) {
  const reload = vi.fn()
  const how: Reload = {
    build,
    storage: {
      getItem: (key) => kept.get(key) ?? null,
      setItem: (key, value) => void kept.set(key, value),
    },
    now: () => now,
    reload,
  }
  return { how, reload }
}

beforeEach(() => {
  staleBuild.value = false
})

describe('a tab whose build is gone from the server', () => {
  it('reloads once for its build, not again right after, and for the next build again', () => {
    const kept = new Map<string, string>()
    const first = tab(kept, '/assets/index-old.js', 1_000)
    expect(reloadForNewBuild(first.how)).toBe(true)
    expect(first.reload).toHaveBeenCalledOnce()
    expect(JSON.parse(kept.get(RELOADED_KEY)!)).toEqual({
      build: '/assets/index-old.js',
      at: 1_000,
    })

    // The same build came back (the server not updated yet): no loop.
    const again = tab(kept, '/assets/index-old.js', 2_000)
    expect(reloadForNewBuild(again.how)).toBe(false)
    expect(again.reload).not.toHaveBeenCalled()
    // A while later it may try once more.
    const later = tab(kept, '/assets/index-old.js', 1_000 + AGAIN_AFTER_MS)
    expect(reloadForNewBuild(later.how)).toBe(true)

    // The new build, after the next update.
    const next = tab(kept, '/assets/index-new.js', 1_000 + AGAIN_AFTER_MS + 1)
    expect(reloadForNewBuild(next.how)).toBe(true)
  })

  it('never reloads where it cannot note that it did', () => {
    const none = { ...tab(new Map(), 'b', 0).how, storage: undefined }
    expect(reloadForNewBuild(none)).toBe(false)
    const reload = vi.fn()
    const full: Reload = {
      build: 'b',
      storage: {
        getItem: () => 'not json',
        setItem: () => {
          throw new Error('quota')
        },
      },
      now: () => 0,
      reload,
    }
    expect(reloadForNewBuild(full)).toBe(false)
    expect(reload).not.toHaveBeenCalled()
  })

  it('hides the error while it reloads, and shows the note when reloading did not help', () => {
    const target = new EventTarget()
    const kept = new Map<string, string>()
    const { how, reload } = tab(kept, '/assets/index-old.js', 5)
    reloadOnStaleBuild(target, () => how)

    const failed = () => {
      const event = new Event('vite:preloadError', { cancelable: true })
      target.dispatchEvent(event)
      return event
    }
    expect(failed().defaultPrevented).toBe(true)
    expect(reload).toHaveBeenCalledOnce()
    expect(staleBuild.value).toBe(false)

    expect(failed().defaultPrevented, 'the error goes on').toBe(false)
    expect(reload).toHaveBeenCalledOnce()
    expect(staleBuild.value).toBe(true)
  })
})
