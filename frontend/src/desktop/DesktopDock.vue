<script setup lang="ts">
import { LayoutGrid, Monitor, PanelRight } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { type AppDefinition, findApp, SETTINGS_APP } from './apps'
import AppTile from './AppTile.vue'
import DockButton from './DockButton.vue'
import NotificationCenter from './NotificationCenter.vue'
import UserMenu from './UserMenu.vue'
import { useWindows } from './windows'

const props = defineProps<{
  apps: AppDefinition[]
  launcherOpen: boolean
  widgetsOpen: boolean
}>()
const emit = defineEmits<{ launcher: []; widgets: []; signOut: [] }>()

const { t } = useI18n()
const windows = useWindows()

const running = computed(() =>
  windows.state.windows.flatMap((window) => {
    const app = findApp(window.appId)
    return app && props.apps.some((a) => a.id === app.id) ? [{ window, app }] : []
  }),
)

function toggle(appId: string, minimized: boolean): void {
  if (windows.active === appId && !minimized) windows.minimize(appId)
  else windows.focus(appId)
}

function openApp(id: string, section?: string): void {
  const app = props.apps.find((a) => a.id === id)
  if (app) windows.open(app, section)
}
</script>

<!--
  The bar down the left edge: what can be opened and what is open, and at
  its foot the widgets' switch, the notifications and the account. How the
  NAS is doing is the widgets' business. It floats clear of the screen's
  edges, so the wallpaper reads as one surface behind everything.
-->
<template>
  <nav
    :aria-label="t('desktop.dock')"
    class="glass glass-thin z-50 flex w-[72px] shrink-0 flex-col items-center gap-2 rounded-panel py-3"
  >
    <!-- Marked so the launcher knows its own switch from a click elsewhere. -->
    <span data-launcher-toggle class="contents">
      <DockButton :label="t('desktop.launcher')" :pressed="launcherOpen" @click="emit('launcher')">
        <LayoutGrid class="size-5" />
      </DockButton>
    </span>
    <DockButton :label="t('desktop.showDesktop')" @click="windows.minimizeAll()">
      <Monitor class="size-5" />
    </DockButton>

    <span v-if="running.length" class="my-1 h-px w-8 bg-foreground/15" />
    <div
      :aria-label="t('desktop.running')"
      class="flex min-h-0 w-full flex-1 flex-col items-center gap-2 overflow-x-hidden overflow-y-auto px-2"
    >
      <DockButton
        v-for="{ window, app } in running"
        :key="app.id"
        :label="t(`apps.${app.id}`)"
        running
        :active="windows.active === app.id && !window.minimized"
        @click="toggle(app.id, window.minimized)"
      >
        <AppTile :app="app" size="dock" />
      </DockButton>
    </div>

    <span class="my-1 h-px w-8 bg-foreground/15" />
    <DockButton :label="t('desktop.widgets')" :pressed="widgetsOpen" @click="emit('widgets')">
      <PanelRight class="size-5" />
    </DockButton>
    <NotificationCenter />
    <UserMenu @open-settings="openApp(SETTINGS_APP)" @sign-out="emit('signOut')" />
  </nav>
</template>
