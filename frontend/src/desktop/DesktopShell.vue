<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { useSessionStore } from '@/features/auth/store'
import { live } from '@/shared/api/live'
import { useCompactLayout } from '@/shared/lib/media'
import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import { useServerNotices } from '@/shared/notices/store'
import { usePreferences } from '@/shared/preferences/store'
import { cn } from '@/shared/lib/utils'
import { IconTile } from '@/shared/ui/icon-tile'

import { APPS, findApp } from './apps'
import AppWindow from './AppWindow.vue'
import PowerOverlay from './PowerOverlay.vue'
import DesktopTaskbar from './DesktopTaskbar.vue'
import ToastStack from './ToastStack.vue'
import { useWindows } from './windows'

const { t } = useI18n()
const router = useRouter()
const session = useSessionStore()
const windows = useWindows()
const notifications = useNotifications()
const jobs = useJobsStore()
const notices = useServerNotices()
const preferences = usePreferences()
const compact = useCompactLayout()

/** Only apps this account may use; the server enforces the same. */
const apps = computed(() => APPS.filter((app) => session.can(app.permission)))

const openWindows = computed(() =>
  windows.state.windows.flatMap((window) => {
    const app = findApp(window.appId)
    return app && session.can(app.permission) ? [{ window, app }] : []
  }),
)

/** Stacking positions 0, 1, 2, … instead of ever-growing z values. */
const layers = computed(
  () =>
    new Map(
      [...windows.state.windows].sort((a, b) => a.z - b.z).map((window, i) => [window.appId, i]),
    ),
)

onMounted(() => {
  windows.restore(session.info?.account.name ?? '')
  window.addEventListener('resize', windows.resized)
  live.connect()
  void jobs.start()
  // Notifications are about pools, disks and shares (`storage.view`).
  if (session.can('storage.view')) void notices.start()
  void preferences.load()
})

onBeforeUnmount(() => {
  window.removeEventListener('resize', windows.resized)
  jobs.stop()
  notices.stop()
  live.disconnect()
})

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
  <div class="wallpaper flex h-dvh flex-col overflow-hidden">
    <main class="relative min-h-0 flex-1">
      <ul
        :class="
          cn(
            compact
              ? 'grid grid-cols-4 gap-2 p-4'
              : 'absolute top-3 left-3 flex max-h-[calc(100%-1.5rem)] flex-col flex-wrap gap-0.5',
          )
        "
      >
        <li v-for="app in apps" :key="app.id">
          <button
            type="button"
            class="group flex w-[84px] flex-col items-center gap-1.5 rounded-lg px-1 py-2 text-xs font-medium text-white outline-none hover:bg-white/10 focus-visible:ring-2 focus-visible:ring-white/60"
            @click="windows.open(app)"
          >
            <IconTile
              :icon="app.icon"
              :tone="app.tone"
              size="xl"
              class="transition-transform group-active:scale-95"
            />
            <span class="text-center leading-tight [text-shadow:0_1px_3px_rgb(0_0_0/0.6)]">
              {{ t(`apps.${app.id}`) }}
            </span>
          </button>
        </li>
      </ul>
      <AppWindow
        v-for="{ window, app } in openWindows"
        :key="app.id"
        :app="app"
        :window="window"
        :layer="layers.get(app.id) ?? 0"
        :active="windows.active === app.id"
        :compact="compact"
      />
    </main>
    <DesktopTaskbar :apps="apps" :compact="compact" @open="windows.open" @sign-out="signOut" />
    <ToastStack />
    <PowerOverlay />
  </div>
</template>
