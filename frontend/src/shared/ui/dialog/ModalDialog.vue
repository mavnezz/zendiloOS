<script setup lang="ts">
import { X } from 'lucide-vue-next'
import {
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogOverlay,
  DialogPortal,
  DialogRoot,
  DialogTitle,
} from 'reka-ui'

defineProps<{ title: string; description?: string; closeLabel: string }>()
const open = defineModel<boolean>('open', { default: false })
</script>

<!-- A modal window for a short form. -->
<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="fixed inset-0 z-[1000] bg-black/40 backdrop-blur-[2px]" />
      <DialogContent
        class="fixed top-1/2 left-1/2 z-[1001] grid max-h-[calc(100dvh-2rem)] w-[calc(100%-2rem)] max-w-md -translate-x-1/2 -translate-y-1/2 gap-4 overflow-y-auto rounded-xl border bg-card p-6 shadow-lg"
        v-bind="description ? {} : { 'aria-describedby': undefined }"
      >
        <div class="flex items-start gap-2">
          <div class="grid flex-1 gap-1">
            <DialogTitle class="text-lg font-semibold">{{ title }}</DialogTitle>
            <DialogDescription v-if="description" class="text-sm text-muted-foreground">
              {{ description }}
            </DialogDescription>
          </div>
          <DialogClose
            :aria-label="closeLabel"
            class="rounded-md p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <X class="size-4" />
          </DialogClose>
        </div>
        <slot />
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
