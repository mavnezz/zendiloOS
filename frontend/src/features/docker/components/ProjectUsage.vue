<script setup lang="ts">
import { computed, ref, toRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { formatBytes, formatRate } from '@/shared/lib/format'
import { TimeChart } from '@/shared/ui/chart'
import { NativeSelect } from '@/shared/ui/native-select'

import { useUsage } from '../api'
import { usageSeries } from '../lib/usage'

/** The project's containers, by name. */
const props = defineProps<{ containers: string[] }>()

const { t } = useI18n()
const range = ref(900)
const { data: moments } = useUsage(range, toRef(props, 'containers'))

const charts = computed(() =>
  (
    [
      { metric: 'cpu', format: (value: number) => `${value.toFixed(1)} %`, base: 10 },
      { metric: 'memory', format: formatBytes, base: 1024 },
      { metric: 'sent', format: formatRate, base: 1024 },
      { metric: 'received', format: formatRate, base: 1024 },
    ] as const
  ).map((chart) => ({
    ...chart,
    series: usageSeries(moments.value ?? [], chart.metric),
  })),
)
const empty = computed(() => charts.value.every((chart) => !chart.series.length))
</script>

<!-- What each of a project's containers used over time, four measures side by side. -->
<template>
  <div class="grid gap-3">
    <div class="flex justify-end">
      <NativeSelect v-model.number="range" class="w-40" :aria-label="t('docker.usage.range')">
        <option :value="900">{{ t('docker.usage.last', { n: 15 }) }}</option>
        <option :value="3600">{{ t('docker.usage.lastHours', { n: 1 }, 1) }}</option>
        <option :value="6 * 3600">{{ t('docker.usage.lastHours', { n: 6 }, 6) }}</option>
        <option :value="12 * 3600">{{ t('docker.usage.lastHours', { n: 12 }, 12) }}</option>
      </NativeSelect>
    </div>
    <p v-if="empty" class="py-10 text-center text-sm text-muted-foreground">
      {{ t('docker.usage.none') }}
    </p>
    <div v-else class="grid gap-3 @3xl:grid-cols-2">
      <section
        v-for="chart in charts"
        :key="chart.metric"
        class="grid gap-2 rounded-xl border bg-card p-4"
        :aria-label="t(`docker.usage.${chart.metric}`)"
      >
        <h3 class="text-sm font-medium">{{ t(`docker.usage.${chart.metric}`) }}</h3>
        <TimeChart
          :series="chart.series"
          :capacity="Math.max(2, (moments ?? []).length)"
          :interval="20"
          :base="chart.base"
          :format="chart.format"
          :height="160"
        />
      </section>
    </div>
  </div>
</template>
