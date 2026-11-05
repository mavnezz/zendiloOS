import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'

import { APPS, type AppDefinition } from './apps'
import {
  activeWindow,
  closeWindow,
  type DesktopState,
  emptyDesktop,
  focusWindow,
  minimizeWindow,
  moveWindow,
  openWindow,
  resizeWindow,
  restoreDesktop,
  type Size,
  toggleMaximized,
} from './windowManager'

/** Height of the taskbar below the desktop area. */
export const TASKBAR_HEIGHT = 48

function currentViewport(): Size {
  return { width: window.innerWidth, height: Math.max(window.innerHeight - TASKBAR_HEIGHT, 0) }
}

/** The open windows, remembered per account in this browser. */
export const useWindows = defineStore('windows', () => {
  const state = ref<DesktopState>(emptyDesktop)
  const viewport = ref<Size>(currentViewport())
  let storageKey: string | null = null

  const active = computed(() => activeWindow(state.value))

  /** Back to how `account` left the desktop in this browser. */
  function restore(account: string): void {
    storageKey = `zendiloos.desktop.${account}`
    let stored: unknown = null
    try {
      stored = JSON.parse(localStorage.getItem(storageKey) ?? 'null')
    } catch {
      stored = null
    }
    const known = new Set<string>(APPS.map((app) => app.id))
    state.value = restoreDesktop(stored, known)
  }

  function forget(): void {
    storageKey = null
    state.value = emptyDesktop
  }

  watch(
    state,
    (value) => {
      if (!storageKey) return
      try {
        localStorage.setItem(storageKey, JSON.stringify(value))
      } catch {
        // Storage unavailable: the layout lasts until the page closes.
      }
    },
    { deep: true },
  )

  /** The windows follow on screen; what is stored stays as the user left it. */
  function resized(): void {
    viewport.value = currentViewport()
  }

  return {
    state,
    viewport,
    active,
    restore,
    forget,
    resized,
    open: (app: AppDefinition) =>
      (state.value = openWindow(state.value, app.id, app.size, viewport.value)),
    close: (appId: string) => (state.value = closeWindow(state.value, appId)),
    focus: (appId: string) => (state.value = focusWindow(state.value, appId)),
    minimize: (appId: string) => (state.value = minimizeWindow(state.value, appId)),
    toggleMaximized: (appId: string) => (state.value = toggleMaximized(state.value, appId)),
    move: (appId: string, position: { x: number; y: number }) =>
      (state.value = moveWindow(state.value, appId, position, viewport.value)),
    resize: (appId: string, size: Size, minimum: Size) =>
      (state.value = resizeWindow(state.value, appId, size, minimum, viewport.value)),
  }
})
