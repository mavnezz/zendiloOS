<script setup lang="ts">
import { ArrowDown, ArrowUp, LayoutGrid, Monitor, PanelRight, Search } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { formatPercent, formatRate } from '@/shared/lib/format'
import { Gauge } from '@/shared/ui/gauge'

import { ACCOUNT_SECTION, type AppDefinition, findApp, SETTINGS_APP } from './apps'
import AppTile from './AppTile.vue'
import DockButton from './DockButton.vue'
import { memoryRatio, networkRates, useLatestMetrics } from './liveMetrics'
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
const latest = useLatestMetrics()

const running = computed(() =>
  windows.state.windows.flatMap((window) => {
    const app = findApp(window.appId)
    return app && props.apps.some((a) => a.id === app.id) ? [{ window, app }] : []
  }),
)
const memory = computed(() => (latest.value ? memoryRatio(latest.value) : 0))
const net = computed(() => (latest.value ? networkRates(latest.value) : null))

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
  The bar down the left edge: what can be opened, what is open, and how the
  NAS is doing. It floats clear of the screen's edges, so the wallpaper
  reads as one surface behind everything.
-->
<template>
  <nav
    :aria-label="t('desktop.dock')"
    class="glass glass-thin z-50 flex w-[72px] shrink-0 flex-col items-center gap-2 rounded-panel py-3"
  >
    <DockButton :label="t('desktop.launcher')" :pressed="launcherOpen" @click="emit('launcher')">
      <LayoutGrid class="size-5" />
    </DockButton>
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
    <div v-if="latest" class="flex flex-col items-center gap-2">
      <DockButton fit :label="t('desktop.resources')" @click="openApp('dashboard')">
        <span class="flex flex-col items-center gap-1">
          <span class="flex flex-col items-center gap-0.5">
            <Gauge
              :value="latest.cpu_percent / 100"
              :label="t('desktop.cpuUsage')"
              :value-text="formatPercent(latest.cpu_percent)"
            />
            <span class="text-[0.5rem] leading-none font-medium text-foreground/60">CPU</span>
          </span>
          <span class="flex flex-col items-center gap-0.5">
            <Gauge
              :value="memory"
              :label="t('desktop.memoryUsage')"
              :value-text="formatPercent(memory * 100)"
            />
            <span class="text-[0.5rem] leading-none font-medium text-foreground/60">RAM</span>
          </span>
        </span>
        <template #tooltip>
          <span class="grid grid-cols-[auto_auto] gap-x-3 tabular-nums">
            <span>CPU</span><span class="text-right">{{ formatPercent(latest.cpu_percent) }}</span>
            <span>RAM</span><span class="text-right">{{ formatPercent(memory * 100) }}</span>
          </span>
        </template>
      </DockButton>
      <div
        v-if="net"
        :aria-label="t('desktop.network')"
        class="flex flex-col items-center gap-0.5 text-[0.5625rem] leading-none text-foreground/70 tabular-nums"
      >
        <span class="flex items-center gap-0.5"
          ><ArrowUp class="size-2.5" />{{ formatRate(net.sent) }}</span
        >
        <span class="flex items-center gap-0.5"
          ><ArrowDown class="size-2.5" />{{ formatRate(net.received) }}</span
        >
      </div>
    </div>
    <span class="my-1 h-px w-8 bg-foreground/15" />

    <DockButton :label="t('desktop.widgets')" :pressed="widgetsOpen" @click="emit('widgets')">
      <PanelRight class="size-5" />
    </DockButton>
    <NotificationCenter />
    <DockButton :label="t('desktop.search')" @click="emit('launcher')">
      <Search class="size-5" />
    </DockButton>
    <UserMenu @open-settings="openApp(SETTINGS_APP, ACCOUNT_SECTION)" @sign-out="emit('signOut')" />
  </nav>
</template>
