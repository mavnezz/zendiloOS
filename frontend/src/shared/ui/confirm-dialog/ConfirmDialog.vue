<script setup lang="ts">
import {
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogOverlay,
  AlertDialogPortal,
  AlertDialogRoot,
  AlertDialogTitle,
} from 'reka-ui'

import { buttonVariants } from '@/shared/ui/button'

defineProps<{
  title: string
  description: string
  confirmLabel: string
  cancelLabel: string
  /** For actions that stop or delete something. */
  destructive?: boolean
}>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ confirm: [] }>()

function confirm(): void {
  emit('confirm')
  open.value = false
}
</script>

<!-- Asks before an action that cannot simply be undone. -->
<template>
  <AlertDialogRoot v-model:open="open">
    <AlertDialogPortal>
      <AlertDialogOverlay class="fixed inset-0 z-[1000] bg-black/40 backdrop-blur-[2px]" />
      <AlertDialogContent
        class="fixed top-1/2 left-1/2 z-[1001] grid w-[calc(100%-2rem)] max-w-md -translate-x-1/2 -translate-y-1/2 gap-4 rounded-xl border bg-card p-6 shadow-lg"
      >
        <AlertDialogTitle class="text-lg font-semibold">{{ title }}</AlertDialogTitle>
        <AlertDialogDescription class="text-sm text-muted-foreground">
          {{ description }}
        </AlertDialogDescription>
        <slot />
        <div class="flex justify-end gap-2">
          <AlertDialogCancel :class="buttonVariants({ variant: 'outline' })">
            {{ cancelLabel }}
          </AlertDialogCancel>
          <!-- Says confirm first, then closes: whoever asked still knows what. -->
          <button
            type="button"
            :class="buttonVariants({ variant: destructive ? 'destructive' : 'default' })"
            @click="confirm"
          >
            {{ confirmLabel }}
          </button>
        </div>
      </AlertDialogContent>
    </AlertDialogPortal>
  </AlertDialogRoot>
</template>
