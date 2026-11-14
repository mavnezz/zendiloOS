<script setup lang="ts">
import { BatteryCharging } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { UpsReading } from '@/shared/api/types'
import { formatDuration, formatPercent } from '@/shared/lib/format'
import { Badge } from '@/shared/ui/badge'
import { PageCard } from '@/shared/ui/page'
import { StatTile, type StatTone } from '@/shared/ui/stat'

/** What the UPS says of itself; none while it does not answer. */
const props = defineProps<{ reading?: UpsReading | null }>()

const { t } = useI18n()

/** The one thing to know: on mains, on battery, or going down. */
const state = computed<{ label: string; variant: 'destructive' | 'warning' | 'success' } | null>(
  () => {
    const reading = props.reading
    if (!reading) return null
    if (reading.shutting_down)
      return { label: t('ups.status.shuttingDown'), variant: 'destructive' }
    if (reading.on_battery) return { label: t('ups.status.onBattery'), variant: 'warning' }
    return { label: t('ups.status.onMains'), variant: 'success' }
  },
)

const tone = computed<StatTone>(() => {
  const reading = props.reading
  if (reading?.shutting_down) return 'destructive'
  return reading?.on_battery || reading?.low_battery ? 'warning' : 'muted'
})
const percent = (value?: number | null) =>
  value === null || value === undefined ? '–' : formatPercent(value)
const model = computed(() =>
  [props.reading?.manufacturer, props.reading?.model].filter(Boolean).join(' '),
)
</script>

<!-- The UPS as it is now: on mains or on battery, and how long the battery lasts. -->
<template>
  <PageCard
    :title="t('ups.status.title')"
    :icon="BatteryCharging"
    :description="model || undefined"
  >
    <template v-if="state" #actions>
      <div class="flex flex-wrap items-center justify-end gap-2" data-testid="ups-state">
        <Badge v-if="reading?.replace_battery" variant="warning">{{
          t('ups.status.replaceBattery')
        }}</Badge>
        <!-- A low battery is said as what it is: a shutdown after a time on
             battery comes with the battery nearly full, and a UPS back on
             mains calls its battery low until it has charged. -->
        <Badge
          v-if="reading?.low_battery"
          :variant="reading.shutting_down ? 'destructive' : 'warning'"
          >{{ t('ups.status.lowBattery') }}</Badge
        >
        <Badge v-if="reading?.charging" variant="outline">{{ t('ups.status.charging') }}</Badge>
        <Badge :variant="state.variant">{{ state.label }}</Badge>
      </div>
    </template>
    <div v-if="!reading" class="grid gap-1" role="status">
      <p class="font-medium">{{ t('ups.status.silent') }}</p>
      <p class="text-muted-foreground">{{ t('ups.status.silentHint') }}</p>
    </div>
    <div v-else class="grid gap-3">
      <div class="grid grid-cols-3 gap-4">
        <StatTile
          :label="t('ups.status.charge')"
          :value="percent(reading.charge_percent)"
          :tone="tone"
        />
        <StatTile
          :label="t('ups.status.runtime')"
          :value="
            reading.runtime_seconds === null || reading.runtime_seconds === undefined
              ? '–'
              : formatDuration(reading.runtime_seconds)
          "
          :hint="t('ups.status.runtimeHint')"
        />
        <StatTile :label="t('ups.status.load')" :value="percent(reading.load_percent)" />
      </div>
      <p class="text-xs text-muted-foreground">
        {{ t('ups.status.says') }}
        <span class="font-mono">{{ reading.status }}</span>
      </p>
    </div>
  </PageCard>
</template>
