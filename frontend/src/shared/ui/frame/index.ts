import type { Component } from 'vue'

export { default as AppFrame } from './AppFrame.vue'
export { default as PanelTabs } from './PanelTabs.vue'

/** One entry of a window's side navigation. */
export interface NavItem {
  id: string
  label: string
  icon?: Component
  /** A number beside the label, e.g. how many are installed. */
  count?: number
  /** Something needs attention there. */
  alert?: boolean
  /** What else a search of the navigation finds it by, besides its name. */
  keywords?: string
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
