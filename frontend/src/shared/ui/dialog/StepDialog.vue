<script setup lang="ts">
import { Check, X } from 'lucide-vue-next'
import {
  DialogClose,
  DialogContent,
  DialogOverlay,
  DialogPortal,
  DialogRoot,
  DialogTitle,
} from 'reka-ui'

import { cn } from '@/shared/lib/utils'

/**
 * `steps` shows numbered steps across the top, `step` the current one
 * (from 0); without steps it is a plain large dialog, e.g. for editing.
 */
defineProps<{ title: string; closeLabel: string; steps?: string[]; step?: number }>()
const open = defineModel<boolean>('open', { default: false })
</script>

<!--
  A large dialog for work in steps, such as creating an account: the steps
  along the top, the content scrolling in between, the buttons always in
  reach at the bottom.
-->
<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="fixed inset-0 z-[1000] bg-black/40 backdrop-blur-[2px]" />
      <DialogContent
        :aria-describedby="undefined"
        class="fixed top-1/2 left-1/2 z-[1001] flex h-[min(40rem,calc(100dvh-2rem))] w-[calc(100%-2rem)] max-w-3xl -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-xl border bg-card shadow-lg"
      >
        <header class="flex items-center gap-2 px-5 pt-4 pb-2">
          <DialogTitle class="flex-1 text-base font-semibold">{{ title }}</DialogTitle>
          <DialogClose
            :aria-label="closeLabel"
            class="rounded-md p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <X class="size-4" />
          </DialogClose>
        </header>
        <ol v-if="steps?.length" class="flex items-center gap-3 px-6 py-3" :aria-label="title">
          <template v-for="(label, index) in steps" :key="label">
            <li
              v-if="index > 0"
              aria-hidden="true"
              :class="cn('h-px min-w-6 flex-1', index <= (step ?? 0) ? 'bg-primary' : 'bg-border')"
            />
            <li
              class="flex shrink-0 items-center gap-2 text-sm"
              :aria-current="index === step ? 'step' : undefined"
            >
              <span
                :class="
                  cn(
                    'grid size-6 place-items-center rounded-full text-xs font-medium tabular-nums',
                    index < (step ?? 0) && 'bg-primary/10 text-primary',
                    index === step && 'bg-primary text-primary-foreground',
                    index > (step ?? 0) && 'bg-muted text-muted-foreground',
                  )
                "
              >
                <Check v-if="index < (step ?? 0)" class="size-3.5" />
                <template v-else>{{ index + 1 }}</template>
              </span>
              <span
                :class="
                  cn(
                    'hidden sm:inline',
                    index === step ? 'font-medium text-foreground' : 'text-muted-foreground',
                  )
                "
                >{{ label }}</span
              >
            </li>
          </template>
        </ol>
        <slot name="tabs" />
        <div class="min-h-0 flex-1 overflow-y-auto px-5 py-3">
          <slot />
        </div>
        <footer class="flex flex-wrap items-center justify-end gap-2 border-t px-5 py-3">
          <slot name="footer" />
        </footer>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
