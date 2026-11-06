import type { Component } from 'vue'

import type { TileTone } from '../icon-tile'

export { default as AppFrame } from './AppFrame.vue'
export { default as PanelTabs } from './PanelTabs.vue'

/** One entry of a window's side navigation. */
export interface NavItem {
  id: string
  label: string
  icon?: Component
  /** Shown as a coloured tile; without it the icon is plain. */
  tone?: TileTone
  /** A number beside the label, e.g. how many are installed. */
  count?: number
  /** Something needs attention there. */
  alert?: boolean
}

export interface NavGroup {
  id: string
  /** Small heading above the group; none for the first, unnamed one. */
  label?: string
  items: NavItem[]
}

export interface TabItem {
  id: string
  label: string
  count?: number
}
