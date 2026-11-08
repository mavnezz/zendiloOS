<script setup lang="ts">
import { X } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { useNotifications } from '@/shared/lib/notifications'
import { cn } from '@/shared/lib/utils'

const { t } = useI18n()
const notifications = useNotifications()
</script>

<!-- New notifications, briefly, in the corner; above them what stays a while (the slot). -->
<template>
  <div
    class="pointer-events-none fixed top-4 right-4 z-[950] flex w-80 flex-col gap-2"
    aria-live="polite"
  >
    <slot />
    <TransitionGroup
      enter-from-class="translate-x-4 opacity-0"
      leave-to-class="translate-x-4 opacity-0"
      enter-active-class="transition duration-200"
      leave-active-class="transition duration-200"
    >
      <div
        v-for="toast in notifications.toasts"
        :key="toast.id"
        :class="
          cn(
            'pointer-events-auto flex gap-3 rounded-lg border bg-card p-3 shadow-lg',
            toast.kind === 'error' && 'border-destructive/40',
          )
        "
        :role="toast.kind === 'error' ? 'alert' : 'status'"
      >
        <div class="min-w-0 flex-1">
          <p class="text-sm font-medium">{{ toast.title }}</p>
          <p v-if="toast.body" class="text-sm text-muted-foreground">{{ toast.body }}</p>
        </div>
        <button
          type="button"
          class="self-start rounded p-0.5 hover:bg-accent"
          :aria-label="t('desktop.close')"
          @click="notifications.dismissToast(toast.id)"
        >
          <X class="size-4" />
        </button>
      </div>
    </TransitionGroup>
  </div>
</template>
