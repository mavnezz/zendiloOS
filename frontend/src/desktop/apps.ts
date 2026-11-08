import {
  Activity,
  Bell,
  Blocks,
  Box,
  Boxes,
  Cable,
  Container,
  Database,
  Disc3,
  FolderOpen,
  Gauge,
  HardDrive,
  Info,
  Languages,
  Layers,
  LayoutDashboard,
  LayoutGrid,
  LibraryBig,
  Network,
  Palette,
  RefreshCw,
  ScrollText,
  Settings,
  Settings2,
  Shield,
  ShieldCheck,
  SquareTerminal,
  Store,
  UserRound,
} from 'lucide-vue-next'
import { type Component, defineAsyncComponent } from 'vue'

import type { Size } from './windowManager'

export type AppId = 'settings' | 'files' | 'apps' | 'docker'

/** A page inside an app: found by the launcher's search, opened directly. */
export interface AppSection {
  id: string
  /** Translation key of its name. */
  label: string
  icon: Component
  /** Translation key of the heading it is listed under. */
  group?: string
  /** Accounts without it do not see the page. */
  permission?: string
  /** Planned: listed, the page says what is coming. */
  soon?: boolean
}

export interface AppDefinition {
  id: AppId
  icon: Component
  /** Background of the app's tile. */
  tile: string
  /** Accounts without it do not see the app. The server checks anyway. */
  permission: string
  /** Shown only once this is on the NAS (installed from the App Store). */
  requires?: 'docker'
  /** Size of a newly opened window. */
  size: Size
  minimum: Size
  /** Pages, for apps with a navigation. */
  sections?: AppSection[]
  /** Loaded when the app is first opened. */
  component: Component
}

export const SETTINGS_APP: AppId = 'settings'
/** The resource monitor's page in the control panel. */
export const MONITOR_SECTION = 'monitor'

/** The control panel's pages, grouped as its navigation shows them. */
export const SETTINGS_SECTIONS: AppSection[] = [
  { id: 'device', label: 'controlPanel.device', icon: Info, group: 'controlPanel.groupGeneral' },
  {
    id: 'appearance',
    label: 'controlPanel.appearance',
    icon: Palette,
    group: 'controlPanel.groupGeneral',
  },
  {
    id: 'network',
    label: 'controlPanel.network',
    icon: Network,
    group: 'controlPanel.groupGeneral',
  },
  {
    id: 'locale',
    label: 'controlPanel.locale',
    icon: Languages,
    group: 'controlPanel.groupGeneral',
  },
  {
    id: 'update',
    label: 'controlPanel.update',
    icon: RefreshCw,
    group: 'controlPanel.groupGeneral',
    permission: 'system.manage',
  },
  {
    id: 'storage-overview',
    label: 'storage.pages.overview.title',
    icon: Gauge,
    group: 'controlPanel.groupStorage',
    permission: 'storage.view',
  },
  {
    id: 'storage-volumes',
    label: 'storage.pages.volumes.title',
    icon: Database,
    group: 'controlPanel.groupStorage',
    permission: 'storage.view',
  },
  {
    id: 'storage-pools',
    label: 'storage.pages.pools.title',
    icon: Layers,
    group: 'controlPanel.groupStorage',
    permission: 'storage.view',
  },
  {
    id: 'storage-disks',
    label: 'storage.pages.disks.title',
    icon: HardDrive,
    group: 'controlPanel.groupStorage',
    permission: 'storage.view',
  },
  {
    id: 'accounts',
    label: 'controlPanel.accounts',
    icon: UserRound,
    group: 'controlPanel.groupServices',
    permission: 'users.manage',
  },
  {
    id: 'file-services',
    label: 'controlPanel.fileServices',
    icon: Cable,
    group: 'controlPanel.groupServices',
    permission: 'system.view',
  },
  {
    id: 'terminal',
    label: 'controlPanel.terminal',
    icon: SquareTerminal,
    group: 'controlPanel.groupServices',
    permission: 'system.manage',
  },
  {
    id: MONITOR_SECTION,
    label: 'controlPanel.monitor',
    icon: Activity,
    group: 'controlPanel.groupMonitoring',
    permission: 'system.view',
  },
  {
    id: 'journal',
    label: 'logs.sections.journal',
    icon: ScrollText,
    group: 'controlPanel.groupMonitoring',
    permission: 'logs.view',
  },
  {
    id: 'audit',
    label: 'logs.sections.audit',
    icon: ShieldCheck,
    group: 'controlPanel.groupMonitoring',
    permission: 'logs.view',
  },
  {
    id: 'security',
    label: 'controlPanel.security',
    icon: Shield,
    group: 'controlPanel.groupSecurity',
    permission: 'system.manage',
  },
  {
    id: 'notifications',
    label: 'controlPanel.notifications',
    icon: Bell,
    group: 'controlPanel.groupSecurity',
    permission: 'system.manage',
  },
]

/** Every app of the desktop, in the order of its icons. Features provide the pages; this puts them in windows. */
export const APPS: AppDefinition[] = [
  {
    id: 'settings',
    icon: Settings,
    tile: 'bg-linear-to-br from-sky-400 to-blue-600',
    permission: 'account.self',
    // Room beside the navigation for the storage tables and the monitor's charts.
    size: { width: 1160, height: 760 },
    minimum: { width: 360, height: 320 },
    sections: SETTINGS_SECTIONS,
    component: defineAsyncComponent(() => import('./apps/SettingsApp.vue')),
  },
  {
    id: 'files',
    icon: FolderOpen,
    tile: 'bg-linear-to-br from-yellow-300 to-amber-500',
    permission: 'files.read',
    size: { width: 1100, height: 700 },
    minimum: { width: 360, height: 320 },
    component: defineAsyncComponent(() => import('./apps/FilesApp.vue')),
  },
  {
    id: 'apps',
    icon: Blocks,
    tile: 'bg-linear-to-br from-blue-400 to-blue-700',
    permission: 'containers.manage',
    size: { width: 1120, height: 740 },
    minimum: { width: 360, height: 320 },
    sections: [
      { id: 'store', label: 'appStore.sections.store', icon: Store },
      { id: 'installed', label: 'appStore.sections.installed', icon: LayoutGrid },
      { id: 'sources', label: 'appStore.sections.sources', icon: LibraryBig },
    ],
    component: defineAsyncComponent(() => import('@/features/apps/components/AppsApp.vue')),
  },
  {
    id: 'docker',
    icon: Container,
    tile: 'bg-linear-to-br from-sky-400 to-blue-600',
    permission: 'containers.manage',
    requires: 'docker',
    size: { width: 1120, height: 760 },
    minimum: { width: 360, height: 320 },
    sections: [
      { id: 'overview', label: 'docker.sections.overview', icon: LayoutDashboard },
      { id: 'projects', label: 'docker.sections.projects', icon: Boxes },
      { id: 'containers', label: 'docker.sections.containers', icon: Box },
      { id: 'images', label: 'docker.sections.images', icon: Disc3 },
      { id: 'networks', label: 'docker.sections.networks', icon: Network },
      { id: 'activity', label: 'docker.sections.activity', icon: ScrollText },
      { id: 'management', label: 'docker.sections.management', icon: Settings2 },
    ],
    component: defineAsyncComponent(() => import('@/features/docker/components/DockerApp.vue')),
  },
]

export function findApp(id: string): AppDefinition | undefined {
  return APPS.find((app) => app.id === id)
}
