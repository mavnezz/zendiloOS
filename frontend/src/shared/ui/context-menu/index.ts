import type { Component } from 'vue'

export { default as ContextMenu } from './ContextMenu.vue'

/** One entry of a context menu or of the toolbar that offers the same. */
export interface MenuAction {
  id: string
  label: string
  icon?: Component
  /** Shown in red. */
  destructive?: boolean
  disabled?: boolean
  /** A line before it. */
  separated?: boolean
  /** As shown beside it, e.g. `Ctrl+C`. */
  shortcut?: string
}
