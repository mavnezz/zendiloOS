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

/** `wide` for content such as pictures. */
defineProps<{ title: string; description?: string; closeLabel: string; wide?: boolean }>()
const open = defineModel<boolean>('open', { default: false })
</script>

<!--
  A modal window for a short form. With a `footer`, its body scrolls between
  the title and the buttons, which stay in view; a `header` goes under the
  title and stays in view too, e.g. to say which step of a wizard this is.
-->
<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="fixed inset-0 z-[1000] bg-black/40 backdrop-blur-[2px]" />
      <DialogContent
        class="fixed top-1/2 left-1/2 z-[1001] max-h-[calc(100dvh-2rem)] w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 rounded-xl border bg-card shadow-lg"
        :class="[
          wide ? 'max-w-4xl' : 'max-w-md',
          $slots.footer ? 'flex flex-col' : 'grid gap-4 overflow-y-auto p-6',
        ]"
        v-bind="description ? {} : { 'aria-describedby': undefined }"
      >
        <div
          class="flex items-start gap-2"
          :class="$slots.footer && ($slots.header ? 'px-6 pt-5 pb-2' : 'px-6 pt-5 pb-4')"
        >
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
        <div v-if="$slots.header" :class="$slots.footer && 'px-6 pb-4'">
          <slot name="header" />
        </div>
        <div
          v-if="$slots.footer"
          class="grid min-h-0 flex-1 content-start gap-4 overflow-y-auto px-6 pb-5"
        >
          <slot />
        </div>
        <slot v-else />
        <div
          v-if="$slots.footer"
          class="flex flex-wrap items-center justify-end gap-2 border-t px-6 py-4"
        >
          <slot name="footer" />
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
