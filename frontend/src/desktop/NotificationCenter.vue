<script setup lang="ts">
import { Bell, CircleAlert, CircleCheck, Info, TriangleAlert } from 'lucide-vue-next'
import { PopoverContent, PopoverPortal, PopoverRoot, PopoverTrigger } from 'reka-ui'
import { useI18n } from 'vue-i18n'

import { type NotificationKind, useNotifications } from '@/shared/lib/notifications'
import { useServerNotices } from '@/shared/notices/store'

const { t, locale } = useI18n()
const notifications = useNotifications()
const notices = useServerNotices()

const icons: Record<NotificationKind, unknown> = {
  info: Info,
  success: CircleCheck,
  warning: TriangleAlert,
  error: CircleAlert,
}
const tones: Record<NotificationKind, string> = {
  info: 'text-primary',
  success: 'text-emerald-600 dark:text-emerald-400',
  warning: 'text-amber-600 dark:text-amber-400',
  error: 'text-destructive',
}

function time(at: number): string {
  return new Intl.DateTimeFormat(locale.value, { timeStyle: 'short' }).format(at)
}

function opened(open: boolean): void {
  if (open) return
  notifications.markAllRead()
  void notices.markRead()
}

function clear(): void {
  notifications.clear()
  void notices.clear()
}
</script>

<template>
  <PopoverRoot @update:open="opened">
    <PopoverTrigger
      class="relative grid size-9 place-items-center rounded-md hover:bg-accent data-[state=open]:bg-accent"
      :aria-label="t('desktop.notifications')"
    >
      <Bell class="size-4" />
      <span
        v-if="notifications.unread"
        class="absolute top-1 right-1 grid min-w-4 place-items-center rounded-full bg-destructive px-1 text-[10px] font-semibold text-white"
      >
        {{ notifications.unread }}
      </span>
    </PopoverTrigger>
    <PopoverPortal>
      <PopoverContent
        side="top"
        align="end"
        :side-offset="8"
        class="z-[900] flex max-h-[70vh] w-80 flex-col rounded-xl border bg-card shadow-xl"
      >
        <div class="flex items-center justify-between border-b px-4 py-2.5">
          <h2 class="text-sm font-semibold">{{ t('desktop.notifications') }}</h2>
          <button
            v-if="notifications.items.length"
            type="button"
            class="text-xs text-muted-foreground hover:text-foreground"
            @click="clear"
          >
            {{ t('desktop.clearNotifications') }}
          </button>
        </div>
        <ul v-if="notifications.items.length" class="divide-y overflow-auto">
          <li v-for="item in notifications.items" :key="item.id" class="flex gap-3 px-4 py-3">
            <component
              :is="icons[item.kind]"
              :class="['mt-0.5 size-4 shrink-0', tones[item.kind]]"
            />
            <div class="min-w-0 flex-1">
              <p class="text-sm font-medium">{{ item.title }}</p>
              <p v-if="item.body" class="text-sm text-muted-foreground">{{ item.body }}</p>
            </div>
            <time class="text-xs text-muted-foreground">{{ time(item.at) }}</time>
          </li>
        </ul>
        <p v-else class="px-4 py-8 text-center text-sm text-muted-foreground">
          {{ t('desktop.noNotifications') }}
        </p>
      </PopoverContent>
    </PopoverPortal>
  </PopoverRoot>
</template>
