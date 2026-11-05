<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{
  values: number[]
  /** Top of the scale; defaults to the largest value. */
  max?: number
  /** Number of points the line spans, so a short history starts at the right. */
  capacity?: number
  class?: HTMLAttributes['class']
}>()

const WIDTH = 100
const HEIGHT = 32

const points = computed(() => {
  const capacity = Math.max(props.capacity ?? props.values.length, 2)
  const top = Math.max(props.max ?? Math.max(...props.values, 0), 1e-9)
  const offset = capacity - props.values.length
  return props.values.map((value, index) => {
    const x = ((offset + index) / (capacity - 1)) * WIDTH
    const y = HEIGHT - (Math.min(value, top) / top) * (HEIGHT - 2) - 1
    return `${x.toFixed(2)},${y.toFixed(2)}`
  })
})

const area = computed(() => {
  const line = points.value
  if (line.length < 2) return ''
  const first = line[0]!.split(',')[0]
  const last = line[line.length - 1]!.split(',')[0]
  return `${first},${HEIGHT} ${line.join(' ')} ${last},${HEIGHT}`
})
</script>

<!-- A small line chart of recent values; the color comes from `text-*`. -->
<template>
  <svg
    :viewBox="`0 0 ${WIDTH} ${HEIGHT}`"
    preserveAspectRatio="none"
    aria-hidden="true"
    :class="cn('h-8 w-full text-primary', props.class)"
  >
    <polygon v-if="area" :points="area" fill="currentColor" fill-opacity="0.12" />
    <polyline
      v-if="points.length > 1"
      :points="points.join(' ')"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      vector-effect="non-scaling-stroke"
      stroke-linejoin="round"
    />
  </svg>
</template>
