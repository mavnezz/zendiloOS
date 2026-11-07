import { computed, inject, type InjectionKey, type Ref, ref, type WritableComputedRef } from 'vue'

/** What the window an app runs in offers it: the page it shows, and other apps. */
export interface WindowSection {
  section: Readonly<Ref<string | undefined>>
  navigate: (section: string) => void
  /** Opens (or brings forward) another app, on one of its pages. */
  openApp?: (app: string, section?: string) => void
}

export const WINDOW_SECTION: InjectionKey<WindowSection> = Symbol('window section')

/**
 * How an app opens another one, such as the App Center the Docker app on
 * a project; `null` outside a window (tests), where there is nothing to
 * open.
 */
export function useOpenApp(): ((app: string, section?: string) => void) | null {
  return inject(WINDOW_SECTION, null)?.openApp ?? null
}

/**
 * The page of an app with a navigation: kept by its window, so the launcher
 * and links can open the app on a page and it is there again after a
 * reload. `ids` are the app's pages; anything else shows the first. Outside
 * a window (tests), the page is the component's own.
 */
export function useWindowSection(ids: () => readonly string[]): WritableComputedRef<string> {
  const window = inject(WINDOW_SECTION, null)
  const local = ref<string | undefined>()
  return computed({
    get: () => {
      const known = ids()
      const current = window ? window.section.value : local.value
      return current && known.includes(current) ? current : (known[0] ?? '')
    },
    set: (next) => {
      if (window) window.navigate(next)
      else local.value = next
    },
  })
}
