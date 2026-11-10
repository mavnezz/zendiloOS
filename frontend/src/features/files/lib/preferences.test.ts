import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
  COLLAPSED_KEY,
  storeCollapsed,
  storedCollapsed,
  storedView,
  storeView,
  VIEW_KEY,
} from './preferences'

/** Storage that refuses every access, as in some private windows. */
function refuseStorage(): void {
  const refuse = () => {
    throw new DOMException('denied', 'SecurityError')
  }
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(refuse)
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(refuse)
}

beforeEach(() => localStorage.clear())
afterEach(() => vi.restoreAllMocks())

describe('the view remembered', () => {
  it('is the list until the grid is chosen, and stays chosen', () => {
    expect(storedView()).toBe('list')
    storeView('grid')
    expect(localStorage.getItem(VIEW_KEY)).toBe('grid')
    expect(storedView()).toBe('grid')
    storeView('list')
    expect(storedView()).toBe('list')
  })

  it('falls back to the list for anything else stored', () => {
    localStorage.setItem(VIEW_KEY, 'tiles')
    expect(storedView()).toBe('list')
  })

  it('is the list without storage, and choosing does no harm', () => {
    refuseStorage()
    expect(() => storeView('grid')).not.toThrow()
    expect(storedView()).toBe('list')
  })
})

describe('the sections folded away', () => {
  it('are none at first, and those folded once stay so', () => {
    expect(storedCollapsed()).toEqual(new Set())
    storeCollapsed(new Set(['mine', 'volume:1']))
    expect(storedCollapsed()).toEqual(new Set(['mine', 'volume:1']))
  })

  it('ignore what is stored when it is not a list of names', () => {
    localStorage.setItem(COLLAPSED_KEY, '{broken')
    expect(storedCollapsed()).toEqual(new Set())
    localStorage.setItem(COLLAPSED_KEY, '{"mine":true}')
    expect(storedCollapsed()).toEqual(new Set())
    localStorage.setItem(COLLAPSED_KEY, '["mine", 3, null]')
    expect(storedCollapsed()).toEqual(new Set(['mine']))
  })

  it('are none without storage, and folding does no harm', () => {
    refuseStorage()
    expect(() => storeCollapsed(new Set(['mine']))).not.toThrow()
    expect(storedCollapsed()).toEqual(new Set())
  })
})
