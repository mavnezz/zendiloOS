<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { useSessionStore } from '@/features/auth/store'
import { api, unwrap } from '@/shared/api/client'
import { live } from '@/shared/api/live'
import { useJobsStore } from '@/shared/jobs/store'
import { useCompactLayout } from '@/shared/lib/media'
import { useNotifications } from '@/shared/lib/notifications'
import { useServerNotices } from '@/shared/notices/store'
import { usePreferences } from '@/shared/preferences/store'
import { BrandMark } from '@/shared/ui/brand'

import AppLauncher from './AppLauncher.vue'
import { ACCOUNT_SECTION, APPS, type AppDefinition, findApp, SETTINGS_APP } from './apps'
import AppWindow from './AppWindow.vue'
import DesktopIcons from './DesktopIcons.vue'
import NotificationCenter from './NotificationCenter.vue'
import PowerOverlay from './PowerOverlay.vue'
import DesktopDock from './DesktopDock.vue'
import ToastStack from './ToastStack.vue'
import UserMenu from './UserMenu.vue'
import WidgetPanel from './WidgetPanel.vue'
import { useWindows } from './windows'

const WIDGETS_KEY = 'zendiloos.widgets.open'

const { t } = useI18n()
const router = useRouter()
const session = useSessionStore()
const windows = useWindows()
const notifications = useNotifications()
const jobs = useJobsStore()
const notices = useServerNotices()
const preferences = usePreferences()
const compact = useCompactLayout()

/** Whether Docker is installed: the App Store's question, shared with it. */
const docker = useQuery({
  queryKey: ['apps', 'docker'],
  queryFn: () => unwrap(api.GET('/api/v1/apps/docker')),
  enabled: computed(() => session.can('containers.manage')),
})

/** Only apps this account may use and the NAS has; the server enforces the same. */
function usable(app: AppDefinition): boolean {
  return session.can(app.permission) && (!app.requires || Boolean(docker.data.value?.installed))
}
const apps = computed(() => APPS.filter(usable))

const openWindows = computed(() =>
  windows.state.windows.flatMap((window) => {
    const app = findApp(window.appId)
    return app && usable(app) ? [{ window, app }] : []
  }),
)

/** Stacking positions 0, 1, 2, … instead of ever-growing z values. */
const layers = computed(
  () =>
    new Map(
      [...windows.state.windows].sort((a, b) => a.z - b.z).map((window, i) => [window.appId, i]),
    ),
)

const launcherOpen = ref(false)
/** Whether the widgets show: a convenience of this browser, not a setting. */
const widgetsOpen = ref(readWidgetsOpen())
function readWidgetsOpen(): boolean {
  try {
    return localStorage.getItem(WIDGETS_KEY) !== 'false'
  } catch {
    return true
  }
}
function toggleWidgets(): void {
  widgetsOpen.value = !widgetsOpen.value
  try {
    localStorage.setItem(WIDGETS_KEY, String(widgetsOpen.value))
  } catch {
    // Only a convenience.
  }
}

// Windows live in the desktop's area, between the bar and the widgets.
const area = ref<HTMLElement | null>(null)
let observer: ResizeObserver | null = null

onMounted(() => {
  windows.restore(session.info?.account.name ?? '')
  observer = new ResizeObserver(([entry]) => {
    if (entry) {
      windows.resized({
        width: Math.floor(entry.contentRect.width),
        height: Math.floor(entry.contentRect.height),
      })
    }
  })
  if (area.value) observer.observe(area.value)
  live.connect()
  void jobs.start()
  // Notifications are about pools, disks and shares (`storage.view`).
  if (session.can('storage.view')) void notices.start()
  void preferences.load()
})

onBeforeUnmount(() => {
  observer?.disconnect()
  jobs.stop()
  notices.stop()
  live.disconnect()
})

function open(app: AppDefinition, section?: string): void {
  windows.open(app, section)
}

async function signOut(): Promise<void> {
  try {
    await session.signOut()
  } finally {
    windows.forget()
    notifications.clear()
    await router.push({ name: 'login' })
  }
}
</script>

<template>
  <!-- Phones: a home screen of widgets and icons; apps open full screen. -->
  <div v-if="compact" class="wallpaper fixed inset-0 overflow-y-auto">
    <header class="flex items-center justify-between p-4">
      <BrandMark class="size-8" />
      <div class="glass glass-thin flex items-center rounded-panel">
        <NotificationCenter />
        <UserMenu
          @open-settings="open(findApp(SETTINGS_APP)!, ACCOUNT_SECTION)"
          @sign-out="signOut"
        />
      </div>
    </header>
    <div class="px-4">
      <WidgetPanel wide />
    </div>
    <main
      ref="area"
      :aria-label="t('desktop.apps')"
      class="grid grid-cols-[repeat(auto-fill,minmax(6rem,1fr))] justify-items-center gap-2 p-4"
    >
      <DesktopIcons :apps="apps" class="contents" @open="open" />
    </main>
    <AppWindow
      v-for="{ window, app } in openWindows"
      :key="app.id"
      :app="app"
      :window="window"
      :layer="layers.get(app.id) ?? 0"
      :active="windows.active === app.id"
      compact
    />
  </div>

  <!-- Everything floats clear of the screen's edges, resting on the wallpaper. -->
  <div v-else class="wallpaper fixed inset-0 flex gap-3 p-3">
    <DesktopDock
      :apps="apps"
      :launcher-open="launcherOpen"
      :widgets-open="widgetsOpen"
      @launcher="launcherOpen = !launcherOpen"
      @widgets="toggleWidgets"
      @sign-out="signOut"
    />
    <div class="relative flex min-w-0 flex-1 gap-3">
      <main ref="area" :aria-label="t('desktop.desktop')" class="relative min-w-0 flex-1">
        <DesktopIcons :apps="apps" @open="open" />
        <AppWindow
          v-for="{ window, app } in openWindows"
          :key="app.id"
          :app="app"
          :window="window"
          :layer="layers.get(app.id) ?? 0"
          :active="windows.active === app.id"
          :compact="false"
        />
      </main>
      <WidgetPanel v-if="widgetsOpen" />
      <AppLauncher v-if="launcherOpen" :apps="apps" @close="launcherOpen = false" @open="open" />
    </div>
  </div>
  <ToastStack />
  <PowerOverlay />
</template>
