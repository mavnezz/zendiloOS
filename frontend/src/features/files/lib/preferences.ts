/** How the explorer shows the entries: as rows with their details, or as tiles. */
export type ExplorerView = 'list' | 'grid'

/** Where this browser remembers the view chosen. */
export const VIEW_KEY = 'zendiloos.files.view'
/** Where this browser remembers which sections of the places are folded away. */
export const COLLAPSED_KEY = 'zendiloos.files.collapsedSections'

/** The view chosen last in this browser; the list without one (or without storage). */
export function storedView(): ExplorerView {
  try {
    return window.localStorage.getItem(VIEW_KEY) === 'grid' ? 'grid' : 'list'
  } catch {
    return 'list'
  }
}

export function storeView(view: ExplorerView): void {
  try {
    window.localStorage.setItem(VIEW_KEY, view)
  } catch {
    // Without storage, the explorer opens as a list next time.
  }
}

/** The sections of the places folded away in this browser, by their IDs. */
export function storedCollapsed(): Set<string> {
  try {
    const stored: unknown = JSON.parse(window.localStorage.getItem(COLLAPSED_KEY) ?? '[]')
    return new Set(
      Array.isArray(stored) ? stored.filter((id): id is string => typeof id === 'string') : [],
    )
  } catch {
    return new Set()
  }
}

export function storeCollapsed(collapsed: Set<string>): void {
  try {
    window.localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...collapsed]))
  } catch {
    // Without storage, every section is open again next time.
  }
}
