<script setup lang="ts">
import { computed } from 'vue'

const props = withDefaults(
  defineProps<{
    /** How full the ring is, from 0 to 1. */
    value: number
    /** Diameter in pixels. */
    size?: number
    thickness?: number
    label: string
    /** The value in words, e.g. "42 %". */
    valueText?: string
  }>(),
  { size: 30, thickness: 3, valueText: undefined },
)

const ratio = computed(() =>
  Number.isFinite(props.value) ? Math.min(1, Math.max(0, props.value)) : 0,
)
const radius = computed(() => (props.size - props.thickness) / 2)
const circumference = computed(() => 2 * Math.PI * radius.value)
const tone = computed(() =>
  ratio.value >= 0.9 ? 'text-destructive' : ratio.value >= 0.75 ? 'text-warning' : 'text-primary',
)
</script>

<!-- A ring that fills up, for one live value; amber, then red, as it fills. -->
<template>
  <div
    role="meter"
    :aria-label="label"
    :aria-valuenow="Math.round(ratio * 100)"
    aria-valuemin="0"
    aria-valuemax="100"
    :aria-valuetext="valueText"
    class="relative inline-grid place-items-center"
    :style="{ width: `${size}px`, height: `${size}px` }"
  >
    <svg
      :width="size"
      :height="size"
      :viewBox="`0 0 ${size} ${size}`"
      aria-hidden="true"
      class="-rotate-90"
    >
      <circle
        :cx="size / 2"
        :cy="size / 2"
        :r="radius"
        fill="none"
        :stroke-width="thickness"
        class="stroke-current text-foreground/15"
      />
      <circle
        :cx="size / 2"
        :cy="size / 2"
        :r="radius"
        fill="none"
        :stroke-width="thickness"
        stroke-linecap="round"
        :stroke-dasharray="circumference"
        :stroke-dashoffset="circumference * (1 - ratio)"
        :class="['stroke-current transition-[stroke-dashoffset,color] duration-500', tone]"
      />
    </svg>
    <span
      v-if="$slots.default"
      class="absolute text-[0.5625rem] leading-none font-medium tabular-nums"
    >
      <slot />
    </span>
  </div>
</template>
