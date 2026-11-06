<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

import type { AppDefinition } from './apps'

const SIZES = {
  title: { tile: 'size-5 rounded-md', icon: 'size-3', stroke: 2 },
  dock: { tile: 'size-8 rounded-lg shadow-sm', icon: 'size-4', stroke: 2 },
  result: { tile: 'size-9 rounded-lg', icon: 'size-4', stroke: 2 },
  desktop: { tile: 'size-14 rounded-tile shadow-lg', icon: 'size-7 drop-shadow-sm', stroke: 1.75 },
} as const

withDefaults(
  defineProps<{
    app: Pick<AppDefinition, 'icon' | 'tile'>
    size?: keyof typeof SIZES
    /** A page's icon on the app's tile. */
    icon?: AppDefinition['icon']
    class?: HTMLAttributes['class']
  }>(),
  { size: 'desktop', icon: undefined, class: undefined },
)
</script>

<!-- The rounded, coloured square that stands for an app. -->
<template>
  <span
    aria-hidden="true"
    :class="
      cn(
        'glass-sheen relative flex shrink-0 items-center justify-center text-white ring-1 ring-white/25 ring-inset',
        app.tile,
        SIZES[size].tile,
        $props.class,
      )
    "
  >
    <component
      :is="icon ?? app.icon"
      :class="SIZES[size].icon"
      :stroke-width="SIZES[size].stroke"
    />
  </span>
</template>
