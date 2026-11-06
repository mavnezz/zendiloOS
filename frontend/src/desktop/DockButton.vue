<script setup lang="ts">
import {
  TooltipContent,
  TooltipPortal,
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
} from 'reka-ui'

import { cn } from '@/shared/lib/utils'

defineProps<{
  label: string
  /** The app's window is the one in front. */
  active?: boolean
  /** Shows the "this app is open" mark. */
  running?: boolean
  pressed?: boolean
  /** As large as what it holds, instead of a square. */
  fit?: boolean
}>()
defineEmits<{ click: [] }>()
</script>

<!-- A button of the bar, named in a tooltip beside it. -->
<template>
  <TooltipProvider :delay-duration="300">
    <TooltipRoot>
      <TooltipTrigger as-child>
        <button
          type="button"
          :aria-label="label"
          :aria-pressed="pressed ?? (running ? active : undefined)"
          :class="
            cn(
              'group relative flex shrink-0 items-center justify-center text-foreground/75 outline-none transition',
              fit ? 'rounded-lg p-1' : 'size-11 rounded-tile',
              'hover:bg-foreground/10 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring active:scale-95',
              pressed && 'bg-foreground/10 text-foreground',
            )
          "
          @click="$emit('click')"
        >
          <slot />
          <span
            v-if="running"
            aria-hidden="true"
            :class="
              cn(
                'absolute top-1/2 -left-2 w-[3px] -translate-y-1/2 rounded-full transition-all duration-200',
                active ? 'h-5 bg-primary' : 'h-1.5 bg-foreground/45',
              )
            "
          />
        </button>
      </TooltipTrigger>
      <TooltipPortal>
        <TooltipContent
          side="right"
          :side-offset="10"
          class="z-[950] rounded-md bg-foreground px-2 py-1 text-xs text-background shadow-md"
        >
          <slot name="tooltip">{{ label }}</slot>
        </TooltipContent>
      </TooltipPortal>
    </TooltipRoot>
  </TooltipProvider>
</template>
