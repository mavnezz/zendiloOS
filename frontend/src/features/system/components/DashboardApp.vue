<script setup lang="ts">
import {
  Cpu,
  HardDrive,
  LoaderCircle,
  MemoryStick,
  Network,
  Server,
  Thermometer,
} from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { formatBytes, formatDuration, formatPercent, formatRate } from '@/shared/lib/format'
import { Meter } from '@/shared/ui/meter'
import { Sparkline } from '@/shared/ui/sparkline'

import { useSystemInfo } from '../api'
import { HISTORY, useLiveMetrics } from '../metrics'
import MetricCard from './MetricCard.vue'

const { t } = useI18n()
const { latest, history } = useLiveMetrics()
const { data: info } = useSystemInfo()

const memory = computed(() => latest.value?.memory)
const memoryShare = computed(() =>
  memory.value && memory.value.total_bytes > 0
    ? memory.value.used_bytes / memory.value.total_bytes
    : 0,
)
const load = computed(() => latest.value?.load_average.map((value) => value.toFixed(2)) ?? [])
const uptime = computed(() => latest.value?.uptime_seconds ?? info.value?.uptime_seconds)
</script>

<!-- Live view of the system; the layout follows the window's width. -->
<template>
  <div class="grid gap-3 p-4 @2xl:grid-cols-2 @5xl:grid-cols-3">
    <p
      v-if="!latest"
      class="col-span-full flex items-center gap-2 text-sm text-muted-foreground"
      role="status"
    >
      <LoaderCircle class="size-4 animate-spin" />
      {{ t('dashboard.waiting') }}
    </p>

    <MetricCard :title="t('dashboard.system')" :icon="Server">
      <dl v-if="info" class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
        <dt class="text-muted-foreground">{{ t('dashboard.host') }}</dt>
        <dd class="truncate font-medium">{{ info.hostname }}</dd>
        <dt class="text-muted-foreground">{{ t('dashboard.os') }}</dt>
        <dd class="truncate">{{ info.os_name ?? 'Linux' }}</dd>
        <dt class="text-muted-foreground">{{ t('dashboard.kernel') }}</dt>
        <dd class="truncate">{{ info.kernel_release }}</dd>
        <dt class="text-muted-foreground">{{ t('dashboard.processor') }}</dt>
        <dd class="truncate">
          {{ info.cpu_model ?? info.architecture }} ·
          {{ t('dashboard.threads', { count: info.cpu_threads }) }}
        </dd>
        <template v-if="uptime !== undefined">
          <dt class="text-muted-foreground">{{ t('dashboard.uptime') }}</dt>
          <dd>{{ formatDuration(uptime) }}</dd>
        </template>
      </dl>
    </MetricCard>

    <MetricCard :title="t('dashboard.cpu')" :icon="Cpu">
      <template v-if="latest">
        <p class="text-3xl font-semibold tabular-nums">{{ formatPercent(latest.cpu_percent) }}</p>
        <Sparkline :values="history.cpu" :max="100" :capacity="HISTORY" />
        <p class="text-xs text-muted-foreground">
          {{ t('dashboard.load', { one: load[0], five: load[1], fifteen: load[2] }) }}
        </p>
      </template>
    </MetricCard>

    <MetricCard :title="t('dashboard.memory')" :icon="MemoryStick">
      <template v-if="memory">
        <p class="text-3xl font-semibold tabular-nums">{{ formatPercent(memoryShare * 100) }}</p>
        <Meter :value="memoryShare" :label="t('dashboard.memory')" />
        <p class="text-xs text-muted-foreground">
          {{
            t('dashboard.memoryUsed', {
              used: formatBytes(memory.used_bytes),
              total: formatBytes(memory.total_bytes),
            })
          }}
        </p>
        <Sparkline :values="history.memory" :max="memory.total_bytes" :capacity="HISTORY" />
      </template>
    </MetricCard>

    <MetricCard :title="t('dashboard.network')" :icon="Network">
      <template v-if="latest">
        <ul v-if="latest.network.length" class="grid gap-1 text-sm">
          <li v-for="item in latest.network" :key="item.name" class="flex items-center gap-2">
            <span class="flex-1 truncate font-medium">{{ item.name }}</span>
            <span class="tabular-nums">↓ {{ formatRate(item.received_bytes_per_second) }}</span>
            <span class="text-muted-foreground tabular-nums">
              ↑ {{ formatRate(item.sent_bytes_per_second) }}
            </span>
          </li>
        </ul>
        <p v-else class="text-sm text-muted-foreground">{{ t('dashboard.noInterfaces') }}</p>
        <div class="grid grid-cols-2 gap-3 text-xs text-muted-foreground">
          <div>
            {{ t('dashboard.received') }}
            <Sparkline :values="history.received" :capacity="HISTORY" />
          </div>
          <div>
            {{ t('dashboard.sent') }}
            <Sparkline
              :values="history.sent"
              :capacity="HISTORY"
              class="text-emerald-600 dark:text-emerald-400"
            />
          </div>
        </div>
      </template>
    </MetricCard>

    <MetricCard :title="t('dashboard.disks')" :icon="HardDrive">
      <template v-if="latest">
        <ul v-if="latest.disks.length" class="grid gap-1 text-sm">
          <li v-for="disk in latest.disks" :key="disk.name" class="flex items-center gap-2">
            <span class="flex-1 truncate font-medium">{{ disk.name }}</span>
            <span class="tabular-nums">{{ formatRate(disk.read_bytes_per_second) }}</span>
            <span class="text-muted-foreground tabular-nums">
              {{ formatRate(disk.written_bytes_per_second) }}
            </span>
          </li>
        </ul>
        <p v-else class="text-sm text-muted-foreground">{{ t('dashboard.noDisks') }}</p>
        <div class="grid grid-cols-2 gap-3 text-xs text-muted-foreground">
          <div>
            {{ t('dashboard.read') }}
            <Sparkline :values="history.read" :capacity="HISTORY" />
          </div>
          <div>
            {{ t('dashboard.written') }}
            <Sparkline
              :values="history.written"
              :capacity="HISTORY"
              class="text-emerald-600 dark:text-emerald-400"
            />
          </div>
        </div>
      </template>
    </MetricCard>

    <MetricCard :title="t('dashboard.temperatures')" :icon="Thermometer">
      <template v-if="latest">
        <ul v-if="latest.temperatures.length" class="grid gap-1 text-sm">
          <li v-for="sensor in latest.temperatures" :key="sensor.label" class="flex gap-2">
            <span class="flex-1 truncate">{{ sensor.label }}</span>
            <span class="font-medium tabular-nums">{{ sensor.celsius.toFixed(0) }} °C</span>
          </li>
        </ul>
        <p v-else class="text-sm text-muted-foreground">{{ t('dashboard.noSensors') }}</p>
      </template>
    </MetricCard>
  </div>
</template>
