<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{
  /** Fill level from 0 to 1. */
  value: number
  label: string
  /** `usage` turns amber and red when nearly full; `progress` stays neutral. */
  kind?: 'usage' | 'progress'
  class?: HTMLAttributes['class']
}>()

const percent = computed(() => Math.round(Math.min(Math.max(props.value, 0), 1) * 100))
const tone = computed(() => {
  if (props.kind === 'progress') return 'bg-primary'
  return percent.value >= 90
    ? 'bg-destructive'
    : percent.value >= 75
      ? 'bg-amber-500'
      : 'bg-primary'
})
</script>

<template>
  <div
    role="meter"
    :aria-label="label"
    aria-valuemin="0"
    aria-valuemax="100"
    :aria-valuenow="percent"
    :class="cn('h-2 w-full overflow-hidden rounded-full bg-muted', props.class)"
  >
    <div
      :class="cn('h-full rounded-full transition-[width]', tone)"
      :style="{ width: `${percent}%` }"
    />
  </div>
</template>
