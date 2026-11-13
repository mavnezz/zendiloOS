<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useId } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'

import type { ChartSeries } from '.'
import { divisions, monotonePath, niceMax } from './scale'

const props = withDefaults(
  defineProps<{
    series: ChartSeries[]
    /** Samples across the chart; a shorter history starts at the right. */
    capacity: number
    /** Seconds between two samples, for the time axis. */
    interval?: number
    /** Top of the scale; otherwise a round number above the highest value. */
    max?: number
    /** Round the scale in powers of 1024 (bytes) or 10. */
    base?: 10 | 1024
    format?: (value: number) => string
    height?: number
    legend?: boolean
  }>(),
  {
    interval: 2,
    max: undefined,
    base: 10,
    format: (value: number) => value.toFixed(0),
    height: 180,
    legend: true,
  },
)

const MARGIN = { top: 8, right: 8, bottom: 22, left: 52 }
const { t } = useI18n()

const element = ref<HTMLElement | null>(null)
const width = ref(0)
let observer: ResizeObserver | null = null
onMounted(() => {
  observer = new ResizeObserver(([entry]) => (width.value = entry?.contentRect.width ?? 0))
  if (element.value) observer.observe(element.value)
})
onBeforeUnmount(() => observer?.disconnect())

const id = useId()
const plot = computed(() => ({
  width: Math.max(width.value - MARGIN.left - MARGIN.right, 10),
  height: props.height - MARGIN.top - MARGIN.bottom,
}))
const top = computed(
  () =>
    props.max ??
    niceMax(Math.max(...props.series.flatMap((series) => series.values), 0) * 1.05, props.base),
)
const ticks = computed(() => {
  const steps = divisions(top.value)
  return Array.from({ length: steps + 1 }, (_, i) => (i / steps) * top.value)
})

const x = (index: number, length: number) =>
  MARGIN.left +
  ((props.capacity - length + index) / Math.max(props.capacity - 1, 1)) * plot.value.width
const y = (value: number) =>
  MARGIN.top +
  plot.value.height -
  (Math.min(Math.max(value, 0), top.value) / top.value) * plot.value.height

const lines = computed(() =>
  props.series.map((series, i) => {
    const points = series.values.map((value, index) => ({
      x: x(index, series.values.length),
      y: y(value),
    }))
    const line = monotonePath(points)
    const first = points[0]
    const last = points.at(-1)
    const bottom = MARGIN.top + plot.value.height
    const area =
      first && last
        ? `${line} L${last.x.toFixed(1)},${bottom} L${first.x.toFixed(1)},${bottom} Z`
        : ''
    return { ...series, line, area, gradient: `${id}-fill-${i}` }
  }),
)

/** Labels under the time axis: minutes before now. */
const timeLabels = computed(() => {
  const total = props.capacity * props.interval
  const minutes = Math.max(Math.round(total / 60), 1)
  const step = minutes > 4 ? 2 : 1
  const labels: { x: number; text: string }[] = []
  for (let m = minutes; m >= 0; m -= step) {
    const share = 1 - (m * 60) / total
    labels.push({
      x: MARGIN.left + share * plot.value.width,
      text: m === 0 ? t('chart.now') : t('chart.minutesAgo', { n: m }),
    })
  }
  return labels
})

// What the pointer is over: the sample nearest to it.
const hover = ref<number | null>(null)
const longest = computed(() => Math.max(...props.series.map((series) => series.values.length), 0))
function pointed(event: PointerEvent): void {
  const box = (event.currentTarget as SVGElement).getBoundingClientRect()
  const share = (event.clientX - box.left - MARGIN.left) / plot.value.width
  const index = Math.round(share * (props.capacity - 1)) - (props.capacity - longest.value)
  hover.value = index >= 0 && index < longest.value ? index : null
}
const hovered = computed(() => {
  if (hover.value === null) return null
  const index = hover.value
  const seconds = (longest.value - 1 - index) * props.interval
  return {
    x: x(index, longest.value),
    ago: seconds === 0 ? t('chart.now') : t('chart.secondsAgo', { n: seconds }),
    values: lines.value.map((series) => {
      const at = index - (longest.value - series.values.length)
      const value = series.values[at]
      return {
        label: series.label,
        color: series.color,
        value,
        y: value === undefined ? null : y(value),
      }
    }),
  }
})
</script>

<!-- Values over the last minutes, with a scale, a time axis and a tooltip. -->
<template>
  <figure class="grid gap-2">
    <div ref="element" class="relative" :style="{ height: `${height}px` }">
      <svg
        v-if="width > 0"
        :width="width"
        :height="height"
        class="block touch-none overflow-visible"
        role="img"
        :aria-label="series.map((s) => s.label).join(', ')"
        @pointermove="pointed"
        @pointerleave="hover = null"
      >
        <defs>
          <linearGradient
            v-for="drawn in lines"
            :id="drawn.gradient"
            :key="drawn.gradient"
            x1="0"
            x2="0"
            y1="0"
            y2="1"
          >
            <stop offset="0%" :stop-color="drawn.color" stop-opacity="0.22" />
            <stop offset="100%" :stop-color="drawn.color" stop-opacity="0" />
          </linearGradient>
        </defs>
        <g class="text-2xs" fill="currentColor">
          <template v-for="tick in ticks" :key="tick">
            <line
              :x1="MARGIN.left"
              :x2="MARGIN.left + plot.width"
              :y1="y(tick)"
              :y2="y(tick)"
              class="stroke-border"
              :stroke-dasharray="tick === 0 ? undefined : '2 3'"
            />
            <text
              :x="MARGIN.left - 6"
              :y="y(tick)"
              text-anchor="end"
              dominant-baseline="middle"
              class="fill-muted-foreground tabular-nums"
            >
              {{ format(tick) }}
            </text>
          </template>
          <text
            v-for="(label, i) in timeLabels"
            :key="label.text"
            :x="label.x"
            :y="height - 6"
            :text-anchor="i === 0 ? 'start' : i === timeLabels.length - 1 ? 'end' : 'middle'"
            class="fill-muted-foreground"
          >
            {{ label.text }}
          </text>
        </g>
        <g v-for="drawn in lines" :key="drawn.label">
          <path
            v-if="drawn.area && !drawn.dashed"
            :d="drawn.area"
            :fill="`url(#${drawn.gradient})`"
          />
          <path
            :d="drawn.line"
            fill="none"
            :stroke="drawn.color"
            stroke-width="1.75"
            stroke-linejoin="round"
            stroke-linecap="round"
            :stroke-dasharray="drawn.dashed ? '4 3' : undefined"
          />
        </g>
        <g v-if="hovered">
          <line
            :x1="hovered.x"
            :x2="hovered.x"
            :y1="MARGIN.top"
            :y2="MARGIN.top + plot.height"
            class="stroke-muted-foreground/40"
          />
          <template v-for="value in hovered.values" :key="value.label">
            <circle
              v-if="value.y !== null"
              :cx="hovered.x"
              :cy="value.y"
              r="3.5"
              :fill="value.color"
              class="stroke-card"
              stroke-width="2"
            />
          </template>
        </g>
      </svg>
      <div
        v-if="hovered"
        :class="
          cn(
            'pointer-events-none absolute top-1 z-10 grid min-w-32 gap-1 rounded-md border bg-card/95 px-2.5 py-1.5 text-xs shadow-md backdrop-blur',
            hovered.x > width / 2 ? '-translate-x-[calc(100%+10px)]' : 'translate-x-2.5',
          )
        "
        :style="{ left: `${hovered.x}px` }"
      >
        <span class="text-muted-foreground">{{ hovered.ago }}</span>
        <span v-for="value in hovered.values" :key="value.label" class="flex items-center gap-1.5">
          <span class="size-2 rounded-full" :style="{ background: value.color }" />
          <span class="flex-1 text-muted-foreground">{{ value.label }}</span>
          <span class="font-medium tabular-nums">{{
            value.value === undefined ? '–' : format(value.value)
          }}</span>
        </span>
      </div>
    </div>
    <figcaption v-if="legend" class="flex flex-wrap gap-x-4 gap-y-1 pl-13 text-xs">
      <span v-for="item in series" :key="item.label" class="flex items-center gap-1.5">
        <span
          :class="
            cn('h-0.5 w-3 rounded-full', item.dashed && 'bg-transparent! border-t-2 border-dashed')
          "
          :style="item.dashed ? { borderColor: item.color } : { background: item.color }"
        />
        {{ item.label }}
      </span>
    </figcaption>
  </figure>
</template>
