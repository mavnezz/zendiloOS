import { Gauge, Settings } from 'lucide-vue-next'
import { type Component, defineAsyncComponent } from 'vue'

import type { Size } from './windowManager'

export type AppId = 'dashboard' | 'settings'

export interface AppDefinition {
  id: AppId
  icon: Component
  /** Accounts without it do not see the app. The server checks anyway. */
  permission: string
  /** Size of a newly opened window. */
  size: Size
  minimum: Size
  /** Loaded when the app is first opened. */
  component: Component
}

/** Every app of the desktop. Features provide the pages; this puts them in windows. */
export const APPS: AppDefinition[] = [
  {
    id: 'dashboard',
    icon: Gauge,
    permission: 'system.view',
    size: { width: 920, height: 640 },
    minimum: { width: 360, height: 280 },
    component: defineAsyncComponent(() => import('@/features/system/components/DashboardApp.vue')),
  },
  {
    id: 'settings',
    icon: Settings,
    permission: 'account.self',
    size: { width: 760, height: 560 },
    minimum: { width: 360, height: 320 },
    component: defineAsyncComponent(() => import('./apps/SettingsApp.vue')),
  },
]

export function findApp(id: string): AppDefinition | undefined {
  return APPS.find((app) => app.id === id)
}
