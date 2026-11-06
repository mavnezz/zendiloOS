<script setup lang="ts">
import { computed, ref, toRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { formatBytes, formatRate } from '@/shared/lib/format'
import { TimeChart } from '@/shared/ui/chart'
import { NativeSelect } from '@/shared/ui/native-select'

import { useUsage } from '../api'
import { type Metric, usageSeries } from '../lib/usage'

/** `containers`: only these; all if empty. */
const props = withDefaults(defineProps<{ containers?: string[]; seconds?: number }>(), {
  containers: () => [],
  seconds: 3600,
})

const { t } = useI18n()
const range = ref(props.seconds)
const { data: moments } = useUsage(range, toRef(props, 'containers'))
const metric = ref<Metric>('cpu')

const series = computed(() => usageSeries(moments.value ?? [], metric.value))
const format = computed(() =>
  metric.value === 'cpu'
    ? (value: number) => `${value.toFixed(1)} %`
    : metric.value === 'memory'
      ? formatBytes
      : formatRate,
)
const tabs = computed(() => [
  { id: 'cpu', label: t('docker.usage.cpu') },
  { id: 'memory', label: t('docker.usage.memory') },
  { id: 'sent', label: t('docker.usage.sent') },
  { id: 'received', label: t('docker.usage.received') },
])
</script>

<!-- What the containers used over the last hours, one line each. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-3">
      <div role="tablist" :aria-label="t('docker.usage.title')" class="flex flex-wrap gap-1">
        <button
          v-for="tab in tabs"
          :key="tab.id"
          type="button"
          role="tab"
          :aria-selected="metric === tab.id"
          class="rounded-md px-3 py-1 text-sm font-medium transition-colors"
          :class="
            metric === tab.id
              ? 'bg-primary/10 text-primary'
              : 'text-muted-foreground hover:bg-accent hover:text-foreground'
          "
          @click="metric = tab.id as Metric"
        >
          {{ tab.label }}
        </button>
      </div>
      <NativeSelect
        v-model.number="range"
        class="ml-auto w-40"
        :aria-label="t('docker.usage.range')"
      >
        <option :value="900">{{ t('docker.usage.last', { n: 15 }) }}</option>
        <option :value="3600">{{ t('docker.usage.lastHours', { n: 1 }, 1) }}</option>
        <option :value="6 * 3600">{{ t('docker.usage.lastHours', { n: 6 }, 6) }}</option>
        <option :value="12 * 3600">{{ t('docker.usage.lastHours', { n: 12 }, 12) }}</option>
      </NativeSelect>
    </div>
    <p v-if="!series.length" class="py-10 text-center text-sm text-muted-foreground">
      {{ t('docker.usage.none') }}
    </p>
    <TimeChart
      v-else
      :series="series"
      :capacity="Math.max(2, (moments ?? []).length)"
      :interval="20"
      :base="metric === 'cpu' ? 10 : 1024"
      :format="format"
      :height="220"
    />
  </div>
</template>
