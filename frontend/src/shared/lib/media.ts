import { onScopeDispose, type Ref, ref } from 'vue'

/** Whether `query` matches, kept up to date. */
export function useMediaQuery(query: string): Ref<boolean> {
  const list = window.matchMedia(query)
  const matches = ref(list.matches)
  const update = (event: MediaQueryListEvent) => (matches.value = event.matches)
  list.addEventListener('change', update)
  onScopeDispose(() => list.removeEventListener('change', update))
  return matches
}

/** Phones and narrow windows: no movable windows, apps take the screen. */
export function useCompactLayout(): Ref<boolean> {
  return useMediaQuery('(max-width: 767px)')
}
