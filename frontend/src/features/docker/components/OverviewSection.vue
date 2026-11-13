<script setup lang="ts">
import { Container } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Gauge } from '@/shared/ui/gauge'

import { useDockerOverview } from '../api'
import UsageChart from './UsageChart.vue'

const { t } = useI18n()
const { data: overview, error } = useDockerOverview()

const memoryShare = computed(() => {
  const o = overview.value
  return o?.memory_bytes != null && o.memory_total_bytes
    ? o.memory_bytes / o.memory_total_bytes
    : null
})
const card = 'rounded-xl border bg-card p-5'
</script>

<!-- The engine at a glance: what runs, what it takes, and its use over time. -->
<template>
  <div class="grid gap-4">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <div class="grid gap-4 @2xl:grid-cols-2">
      <section :class="card" class="grid gap-5 @2xl:col-span-2">
        <div class="flex items-center gap-3">
          <span class="grid size-10 place-items-center rounded-xl bg-primary/10 text-primary">
            <Container class="size-5" />
          </span>
          <div>
            <h3 class="font-semibold">Docker</h3>
            <p class="text-sm" :class="error ? 'text-destructive' : 'text-muted-foreground'">
              {{ error ? t('docker.overview.down') : t('docker.overview.up') }}
            </p>
          </div>
        </div>
        <dl class="grid grid-cols-2 gap-4 @2xl:grid-cols-4">
          <div>
            <dt class="text-xs text-muted-foreground">{{ t('docker.sections.projects') }}</dt>
            <dd class="display text-xl tabular-nums">
              <span>{{ overview?.projects_running ?? '–' }}</span
              ><span class="text-muted-foreground">/{{ overview?.projects ?? '–' }}</span>
            </dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">{{ t('docker.sections.containers') }}</dt>
            <dd class="display text-xl tabular-nums">
              <span>{{ overview?.containers_running ?? '–' }}</span
              ><span class="text-muted-foreground">/{{ overview?.containers ?? '–' }}</span>
            </dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">{{ t('docker.sections.images') }}</dt>
            <dd class="display text-xl tabular-nums">{{ overview?.images ?? '–' }}</dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">{{ t('docker.overview.data') }}</dt>
            <dd class="display text-xl tabular-nums">
              {{ overview ? formatBytes(overview.data_bytes) : '–' }}
            </dd>
          </div>
        </dl>
      </section>
      <section :class="card" class="flex items-center justify-between gap-4">
        <div>
          <h3 class="font-semibold">{{ t('docker.usage.cpu') }}</h3>
          <p class="text-sm text-muted-foreground">{{ t('docker.overview.ofMachine') }}</p>
        </div>
        <Gauge
          :value="(overview?.cpu_share ?? 0) / 100"
          :size="96"
          :thickness="10"
          :label="t('docker.usage.cpu')"
          :value-text="overview?.cpu_share != null ? `${overview.cpu_share.toFixed(0)} %` : '–'"
        >
          <span class="text-lg font-semibold">{{
            overview?.cpu_share != null ? `${overview.cpu_share.toFixed(0)} %` : '–'
          }}</span>
        </Gauge>
      </section>
      <section :class="card" class="flex items-center justify-between gap-4">
        <div>
          <h3 class="font-semibold">{{ t('docker.usage.memory') }}</h3>
          <p class="text-sm text-muted-foreground">
            {{ overview?.memory_bytes != null ? formatBytes(overview.memory_bytes) : '–' }}
            <template v-if="overview?.memory_total_bytes">
              / {{ formatBytes(overview.memory_total_bytes) }}</template
            >
          </p>
        </div>
        <Gauge
          :value="memoryShare ?? 0"
          :size="96"
          :thickness="10"
          :label="t('docker.usage.memory')"
          :value-text="memoryShare != null ? `${(memoryShare * 100).toFixed(0)} %` : '–'"
        >
          <span class="text-lg font-semibold">{{
            memoryShare != null ? `${(memoryShare * 100).toFixed(0)} %` : '–'
          }}</span>
        </Gauge>
      </section>
    </div>
    <section :class="card">
      <h3 class="mb-3 font-semibold">{{ t('docker.usage.title') }}</h3>
      <UsageChart />
    </section>
  </div>
</template>
