<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Disk, EnclosureOrientation, PlacedSlot, SlotForm } from '@/shared/api/types'
import { formatBytes } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'

import { useSmartReport } from '../api'
import { usageLabel } from '../format'
import { slotTone, type SlotTone } from '../lib/enclosure'

const props = defineProps<{
  place: PlacedSlot
  form: SlotForm
  orientation: EnclosureOrientation
  /** Only the colour of its health: the overview's small drawing. */
  compact?: boolean
  /** The disk in it, as the disk list has it. */
  disk?: Disk
  blinking?: boolean
}>()
defineEmits<{ select: [name: string] }>()

const { t } = useI18n()
// The disk list asks for the same reports, so they are shared, not asked twice.
const { data: report } = useSmartReport(
  () => props.place.disk ?? '',
  () => !props.compact && !!props.place.disk,
)

const tone = computed<SlotTone>(() => slotTone(props.place, report.value))
const upright = computed(() => props.orientation === 'vertical' && props.form !== 'm2')

const tones: Record<SlotTone, { tile: string; light: string }> = {
  good: { tile: 'border-success/50 bg-success/[0.07]', light: 'bg-success' },
  warning: { tile: 'border-warning/70 bg-warning/[0.10]', light: 'bg-warning' },
  failing: { tile: 'border-destructive/70 bg-destructive/[0.10]', light: 'bg-destructive' },
  asleep: { tile: 'border-info/50 bg-info/[0.06]', light: 'bg-info' },
  unknown: { tile: 'border-border bg-card', light: 'bg-muted-foreground/40' },
  empty: { tile: 'border-dashed border-border bg-muted/30', light: 'bg-transparent' },
  missing: {
    tile: 'border-dashed border-destructive/60 bg-destructive/[0.04]',
    light: 'bg-transparent',
  },
}

const description = computed(() =>
  t('storage.enclosure.tile', {
    slot: props.place.label,
    disk: props.place.disk ?? '–',
    state: t(`storage.enclosure.tones.${tone.value}`),
  }),
)
</script>

<!--
  One slot of the enclosure: a drive tray with a light whose colour is the
  disk's health; a button when a disk is in it, which opens the disk.
-->
<template>
  <component
    :is="place.disk ? 'button' : 'div'"
    :type="place.disk ? 'button' : undefined"
    :role="compact && !place.disk ? 'img' : undefined"
    :aria-label="compact || place.disk ? description : undefined"
    :title="compact ? description : undefined"
    :data-tone="tone"
    :class="
      cn(
        'relative flex min-w-0 overflow-hidden rounded-md border text-left transition-colors outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50',
        tones[tone].tile,
        place.disk && 'cursor-pointer hover:border-primary/60',
        upright ? 'flex-col' : 'flex-row items-center',
        compact
          ? upright
            ? 'h-14'
            : 'h-6'
          : upright
            ? 'min-h-36'
            : form === 'm2'
              ? 'min-h-12'
              : 'min-h-14',
      )
    "
    @click="place.disk && $emit('select', place.disk)"
  >
    <span
      aria-hidden="true"
      :class="
        cn(
          'shrink-0',
          tones[tone].light,
          upright ? 'h-1.5 w-full' : 'h-full w-1.5 self-stretch',
          blinking && 'animate-pulse motion-reduce:animate-none',
        )
      "
    />
    <span
      v-if="blinking"
      aria-hidden="true"
      class="absolute top-1.5 right-1.5 size-2 animate-ping rounded-full bg-primary motion-reduce:animate-none"
    />
    <span v-if="!compact" class="grid min-w-0 flex-1 content-start gap-0.5 px-2 py-1.5 text-xs">
      <span class="truncate font-semibold">{{ place.label }}</span>
      <template v-if="place.disk">
        <span class="truncate font-mono font-medium">{{ place.disk }}</span>
        <span v-if="disk" class="truncate text-muted-foreground">
          {{ disk.model ?? t('storage.unknownModel') }}
        </span>
        <span v-if="disk" class="truncate text-muted-foreground tabular-nums">
          {{ formatBytes(disk.size_bytes) }}
          <template v-if="report?.temperature_celsius != null">
            · {{ report.temperature_celsius }} °C</template
          >
        </span>
        <span v-if="disk" class="truncate text-muted-foreground">{{ usageLabel(disk) }}</span>
        <span v-if="blinking" class="font-medium text-primary">
          {{ t('storage.enclosure.blinking') }}
        </span>
      </template>
      <span v-else-if="place.state === 'missing'" class="font-medium text-destructive">
        {{ t('storage.enclosure.missing') }}
      </span>
      <span v-else class="text-muted-foreground">{{ t('storage.enclosure.empty') }}</span>
    </span>
  </component>
</template>
