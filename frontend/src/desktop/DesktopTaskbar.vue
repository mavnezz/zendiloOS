<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'

import { type AppDefinition, findApp } from './apps'
import AppLauncher from './AppLauncher.vue'
import NotificationCenter from './NotificationCenter.vue'
import UserMenu from './UserMenu.vue'
import { TASKBAR_HEIGHT, useWindows } from './windows'

defineProps<{ apps: AppDefinition[]; compact: boolean }>()
const emit = defineEmits<{ open: [app: AppDefinition]; signOut: [] }>()

const { t, locale } = useI18n()
const windows = useWindows()

const running = computed(() =>
  windows.state.windows.flatMap((window) => {
    const app = findApp(window.appId)
    return app ? [{ window, app }] : []
  }),
)

function toggle(appId: string): void {
  if (windows.active === appId) windows.minimize(appId)
  else windows.focus(appId)
}

function openSettings(): void {
  const settings = findApp('settings')
  if (settings) emit('open', settings)
}

const now = ref(new Date())
const timer = setInterval(() => (now.value = new Date()), 10_000)
onBeforeUnmount(() => clearInterval(timer))
const clock = computed(() =>
  new Intl.DateTimeFormat(locale.value, { timeStyle: 'short' }).format(now.value),
)
</script>

<template>
  <footer
    class="relative z-[800] flex shrink-0 items-center gap-1 border-t bg-background/85 px-2 backdrop-blur"
    :style="{ height: `${TASKBAR_HEIGHT}px` }"
  >
    <AppLauncher :apps="apps" @open="emit('open', $event)" />
    <div class="mx-1 h-6 w-px bg-border" />
    <nav v-if="!compact" class="flex min-w-0 flex-1 gap-1 overflow-x-auto">
      <button
        v-for="{ window, app } in running"
        :key="app.id"
        type="button"
        :class="
          cn(
            'flex h-9 max-w-44 shrink-0 items-center gap-2 rounded-md px-3 text-sm hover:bg-accent',
            windows.active === app.id && 'bg-accent font-medium',
            window.minimized && 'text-muted-foreground',
          )
        "
        :aria-pressed="windows.active === app.id"
        @click="toggle(app.id)"
      >
        <component :is="app.icon" class="size-4 shrink-0 text-primary" />
        <span class="truncate">{{ t(`apps.${app.id}`) }}</span>
      </button>
    </nav>
    <span v-else class="flex-1" />
    <NotificationCenter />
    <time class="hidden px-2 text-sm tabular-nums sm:block">{{ clock }}</time>
    <UserMenu @open-settings="openSettings" @sign-out="emit('signOut')" />
  </footer>
</template>
