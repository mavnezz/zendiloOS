<script setup lang="ts">
import { type Component, computed, type HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

import { type TileTone, tileSizes, tileTones } from '.'

const props = withDefaults(
  defineProps<{
    icon: Component
    tone?: TileTone
    size?: keyof typeof tileSizes
    class?: HTMLAttributes['class']
  }>(),
  { tone: 'blue', size: 'md' },
)

const size = computed(() => tileSizes[props.size])
</script>

<!-- An icon on a coloured squircle: how apps and sections are told apart at a glance. -->
<template>
  <span
    aria-hidden="true"
    :class="
      cn(
        'relative inline-grid shrink-0 place-items-center bg-linear-to-b text-white',
        'shadow-[inset_0_1px_0_rgb(255_255_255/0.35),0_1px_2px_rgb(0_0_0/0.18)]',
        tileTones[tone],
        size.tile,
        props.class,
      )
    "
  >
    <component :is="icon" :class="size.icon" :stroke-width="size.stroke" />
  </span>
</template>
