<script setup lang="ts">
import { LoaderCircle } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { formatBytes, formatDuration, formatPercent, formatRate } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'
import { TimeChart } from '@/shared/ui/chart'
import { InfoList } from '@/shared/ui/info-list'
import { PageHeader, PageTabs } from '@/shared/ui/page'
import { PanelSection } from '@/shared/ui/section'
import { Sparkline } from '@/shared/ui/sparkline'
import { StatTile } from '@/shared/ui/stat'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useSystemInfo } from '../api'
import { HISTORY, useLiveMetrics } from '../metrics'

type Tab = 'overview' | 'cpu' | 'memory' | 'network' | 'disks'

const { t } = useI18n()
const { latest, history, interfaces, disks } = useLiveMetrics()
const { data: info } = useSystemInfo()

const tab = ref<Tab>('overview')
const tabs = computed(() =>
  (['overview', 'cpu', 'memory', 'network', 'disks'] as const).map((id) => ({
    id,
    label: t(`dashboard.tabs.${id}`),
  })),
)

const sum = (values: number[]) => values.reduce((total, value) => total + value, 0)
const memory = computed(() => latest.value?.memory)
const memoryPercent = computed(() =>
  memory.value && memory.value.total_bytes > 0
    ? (memory.value.used_bytes / memory.value.total_bytes) * 100
    : 0,
)
const load = computed(() => latest.value?.load_average.map((value) => value.toFixed(2)) ?? [])
const uptime = computed(() => latest.value?.uptime_seconds ?? info.value?.uptime_seconds)
const received = computed(() =>
  sum(latest.value?.network.map((n) => n.received_bytes_per_second) ?? []),
)
const sent = computed(() => sum(latest.value?.network.map((n) => n.sent_bytes_per_second) ?? []))
const read = computed(() => sum(latest.value?.disks.map((d) => d.read_bytes_per_second) ?? []))
const written = computed(() =>
  sum(latest.value?.disks.map((d) => d.written_bytes_per_second) ?? []),
)

/** The processor's own sensor, as the kernel's drivers name it. */
const cpuTemperature = computed(() =>
  latest.value?.temperatures.find((sensor) =>
    /package|tctl|tdie|coretemp|k10temp|cpu/i.test(sensor.label),
  ),
)

const system = computed(() => {
  const value = info.value
  if (!value) return []
  const cores = value.cpu_cores ? `${value.cpu_cores} C / ` : ''
  return [
    { label: t('dashboard.host'), value: value.hostname, mono: true },
    { label: t('dashboard.os'), value: value.os_name ?? 'Linux' },
    {
      label: t('dashboard.kernel'),
      value: `${value.kernel_release} · ${value.architecture}`,
      mono: true,
    },
    {
      label: t('dashboard.processor'),
      value: `${value.cpu_model ?? value.architecture} · ${cores}${value.cpu_threads} T`,
    },
    { label: t('dashboard.memory'), value: formatBytes(value.memory_total_bytes) },
    ...(value.board ? [{ label: t('dashboard.board'), value: value.board }] : []),
    ...(value.firmware
      ? [
          {
            label: t('dashboard.firmware'),
            value: `${value.firmware}${value.uefi ? ' · UEFI' : ''}`,
          },
        ]
      : []),
    { label: t('dashboard.version'), value: value.version, mono: true },
    ...(uptime.value !== undefined
      ? [{ label: t('dashboard.uptime'), value: formatDuration(uptime.value) }]
      : []),
  ]
})

/** A temperature's colour: the interface's own while fine, then warm, hot. */
function heat(celsius: number): string {
  if (celsius >= 80) return 'bg-destructive'
  if (celsius >= 65) return 'bg-warning'
  return 'bg-primary'
}

const percent = (value: number) => `${value.toFixed(0)} %`
const chart = { capacity: HISTORY, interval: 2 }
</script>

<!-- What the NAS is doing now and in the last minutes: a page of the control panel. -->
<template>
  <div>
    <PageHeader :title="t('controlPanel.monitor')" :description="t('dashboard.description')">
      <span v-if="uptime !== undefined" class="text-xs text-muted-foreground tabular-nums">
        {{ t('dashboard.upFor', { time: formatDuration(uptime) }) }}
      </span>
    </PageHeader>
    <PageTabs v-model="tab" :tabs="tabs" :label="t('controlPanel.monitor')" />

    <p v-if="!latest" class="flex items-center gap-2 text-sm text-muted-foreground" role="status">
      <LoaderCircle class="size-4 animate-spin" />
      {{ t('dashboard.waiting') }}
    </p>

    <div v-else-if="tab === 'overview'" class="grid gap-5">
      <div class="grid gap-3 @lg:grid-cols-2 @3xl:grid-cols-4">
        <button
          v-for="tile in [
            {
              id: 'cpu',
              label: t('dashboard.cpu'),
              value: formatPercent(latest.cpu_percent),
              hint: t('dashboard.loadShort', { one: load[0] }),
              values: history.cpu,
              max: 100,
              color: 'text-chart-1',
            },
            {
              id: 'memory',
              label: t('dashboard.memory'),
              value: formatPercent(memoryPercent),
              hint: memory
                ? `${formatBytes(memory.used_bytes)} / ${formatBytes(memory.total_bytes)}`
                : '',
              values: history.memory,
              max: memory?.total_bytes,
              color: 'text-chart-5',
            },
            {
              id: 'network',
              label: t('dashboard.network'),
              value: `↓ ${formatRate(received)}`,
              hint: `↑ ${formatRate(sent)}`,
              values: history.received,
              max: undefined,
              color: 'text-chart-2',
            },
            {
              id: 'disks',
              label: t('dashboard.disks'),
              value: `R ${formatRate(read)}`,
              hint: `W ${formatRate(written)}`,
              values: history.read,
              max: undefined,
              color: 'text-chart-3',
            },
          ]"
          :key="tile.id"
          type="button"
          class="grid gap-1 rounded-lg border bg-muted/30 p-3 text-left transition-colors hover:border-primary/40 hover:bg-muted/50"
          @click="tab = tile.id as Tab"
        >
          <span class="text-xs text-muted-foreground">{{ tile.label }}</span>
          <span class="display text-lg leading-6 tabular-nums">{{ tile.value }}</span>
          <span class="truncate text-xs text-muted-foreground tabular-nums">{{ tile.hint }}</span>
          <Sparkline
            :values="tile.values"
            :max="tile.max"
            :capacity="HISTORY"
            :class="cn('mt-1 h-9', tile.color)"
          />
        </button>
      </div>

      <div class="grid gap-5 @3xl:grid-cols-[3fr_2fr]">
        <PanelSection :title="t('dashboard.system')">
          <InfoList :items="system" />
        </PanelSection>
        <PanelSection :title="t('dashboard.temperatures')">
          <ul v-if="latest.temperatures.length" class="grid gap-2 text-sm">
            <li
              v-for="sensor in latest.temperatures"
              :key="sensor.label"
              class="grid grid-cols-[1fr_auto] items-center gap-x-3 gap-y-1"
            >
              <span class="truncate font-mono text-xs text-muted-foreground">{{
                sensor.label
              }}</span>
              <span class="font-medium tabular-nums">{{ sensor.celsius.toFixed(0) }} °C</span>
              <span class="col-span-2 h-1 overflow-hidden rounded-full bg-muted">
                <span
                  :class="cn('block h-full rounded-full', heat(sensor.celsius))"
                  :style="{ width: `${Math.min(sensor.celsius, 100)}%` }"
                />
              </span>
            </li>
          </ul>
          <p v-else class="text-sm text-muted-foreground">{{ t('dashboard.noSensors') }}</p>
        </PanelSection>
      </div>

      <div class="grid gap-5 @3xl:grid-cols-2">
        <PanelSection :title="t('dashboard.interfaces')">
          <Table v-if="latest.network.length">
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('dashboard.device') }}</TableHead>
                <TableHead class="text-right">↓ {{ t('dashboard.received') }}</TableHead>
                <TableHead class="text-right">↑ {{ t('dashboard.sent') }}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="item in latest.network" :key="item.name">
                <TableCell class="font-mono text-xs">{{ item.name }}</TableCell>
                <TableCell class="text-right tabular-nums">{{
                  formatRate(item.received_bytes_per_second)
                }}</TableCell>
                <TableCell class="text-right tabular-nums">{{
                  formatRate(item.sent_bytes_per_second)
                }}</TableCell>
              </TableRow>
            </TableBody>
          </Table>
          <p v-else class="text-sm text-muted-foreground">{{ t('dashboard.noInterfaces') }}</p>
        </PanelSection>
        <PanelSection :title="t('dashboard.disks')">
          <Table v-if="latest.disks.length">
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('dashboard.device') }}</TableHead>
                <TableHead class="text-right">{{ t('dashboard.read') }}</TableHead>
                <TableHead class="text-right">{{ t('dashboard.written') }}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="disk in latest.disks" :key="disk.name">
                <TableCell class="font-mono text-xs">{{ disk.name }}</TableCell>
                <TableCell class="text-right tabular-nums">{{
                  formatRate(disk.read_bytes_per_second)
                }}</TableCell>
                <TableCell class="text-right tabular-nums">{{
                  formatRate(disk.written_bytes_per_second)
                }}</TableCell>
              </TableRow>
            </TableBody>
          </Table>
          <p v-else class="text-sm text-muted-foreground">{{ t('dashboard.noDisks') }}</p>
        </PanelSection>
      </div>
    </div>

    <div v-else-if="tab === 'cpu'" class="grid gap-5">
      <div class="grid grid-cols-2 gap-4 @2xl:grid-cols-4">
        <StatTile
          :label="t('dashboard.usage')"
          :value="formatPercent(latest.cpu_percent)"
          tone="chart-1"
        />
        <StatTile
          :label="t('dashboard.loadAverage')"
          :value="load[0] ?? '–'"
          :hint="t('dashboard.loadLater', { five: load[1], fifteen: load[2] })"
        />
        <StatTile
          :label="t('dashboard.threadsLabel')"
          :value="String(info?.cpu_threads ?? '–')"
          :hint="info?.cpu_cores ? t('dashboard.cores', { count: info.cpu_cores }) : undefined"
        />
        <StatTile
          :label="t('dashboard.temperature')"
          :value="cpuTemperature ? `${cpuTemperature.celsius.toFixed(0)} °C` : '–'"
          :hint="cpuTemperature?.label"
          :tone="cpuTemperature && cpuTemperature.celsius >= 80 ? 'destructive' : 'muted'"
        />
      </div>
      <TimeChart
        v-bind="chart"
        :series="[{ label: t('dashboard.usage'), values: history.cpu, color: 'var(--chart-1)' }]"
        :max="100"
        :format="percent"
        :height="220"
      />
      <p v-if="info?.cpu_model" class="font-mono text-xs text-muted-foreground">
        {{ info.cpu_model }}
      </p>
    </div>

    <div v-else-if="tab === 'memory' && memory" class="grid gap-5">
      <div class="grid grid-cols-2 gap-4 @2xl:grid-cols-4">
        <StatTile
          :label="t('dashboard.used')"
          :value="formatBytes(memory.used_bytes)"
          :hint="formatPercent(memoryPercent)"
          tone="chart-5"
        />
        <StatTile
          :label="t('dashboard.available')"
          :value="formatBytes(memory.available_bytes)"
          tone="chart-2"
        />
        <StatTile :label="t('dashboard.total')" :value="formatBytes(memory.total_bytes)" />
        <StatTile
          :label="t('dashboard.swap')"
          :value="info ? formatBytes(info.swap_total_bytes) : '–'"
        />
      </div>
      <div class="grid gap-1.5">
        <div class="flex h-2 overflow-hidden rounded-full bg-muted">
          <span class="bg-chart-5" :style="{ width: `${memoryPercent}%` }" />
          <span
            class="bg-chart-2/50"
            :style="{ width: `${(memory.available_bytes / memory.total_bytes) * 100}%` }"
          />
        </div>
      </div>
      <TimeChart
        v-bind="chart"
        :series="[{ label: t('dashboard.used'), values: history.memory, color: 'var(--chart-5)' }]"
        :max="memory.total_bytes"
        :format="formatBytes"
        :height="220"
      />
    </div>

    <div v-else-if="tab === 'network'" class="grid gap-6">
      <div class="grid grid-cols-2 gap-4 @2xl:grid-cols-4">
        <StatTile
          :label="`↓ ${t('dashboard.received')}`"
          :value="formatRate(received)"
          tone="chart-1"
        />
        <StatTile :label="`↑ ${t('dashboard.sent')}`" :value="formatRate(sent)" tone="chart-3" />
        <StatTile :label="t('dashboard.interfaces')" :value="String(latest.network.length)" />
      </div>
      <PanelSection :title="t('dashboard.allInterfaces')">
        <TimeChart
          v-bind="chart"
          :series="[
            { label: t('dashboard.received'), values: history.received, color: 'var(--chart-1)' },
            {
              label: t('dashboard.sent'),
              values: history.sent,
              color: 'var(--chart-3)',
              dashed: true,
            },
          ]"
          :base="1024"
          :format="formatRate"
          :height="180"
        />
      </PanelSection>
      <PanelSection v-for="(series, name) in interfaces" :key="name" :title="String(name)">
        <TimeChart
          v-bind="chart"
          :series="[
            { label: t('dashboard.received'), values: series[0], color: 'var(--chart-1)' },
            {
              label: t('dashboard.sent'),
              values: series[1],
              color: 'var(--chart-3)',
              dashed: true,
            },
          ]"
          :base="1024"
          :format="formatRate"
          :height="120"
          :legend="false"
        />
      </PanelSection>
    </div>

    <div v-else-if="tab === 'disks'" class="grid gap-6">
      <div class="grid grid-cols-2 gap-4 @2xl:grid-cols-4">
        <StatTile :label="t('dashboard.read')" :value="formatRate(read)" tone="chart-2" />
        <StatTile :label="t('dashboard.written')" :value="formatRate(written)" tone="chart-4" />
        <StatTile :label="t('dashboard.devices')" :value="String(latest.disks.length)" />
      </div>
      <PanelSection :title="t('dashboard.allDisks')">
        <TimeChart
          v-bind="chart"
          :series="[
            { label: t('dashboard.read'), values: history.read, color: 'var(--chart-2)' },
            {
              label: t('dashboard.written'),
              values: history.written,
              color: 'var(--chart-4)',
              dashed: true,
            },
          ]"
          :base="1024"
          :format="formatRate"
          :height="180"
        />
      </PanelSection>
      <PanelSection v-for="(series, name) in disks" :key="name" :title="String(name)">
        <TimeChart
          v-bind="chart"
          :series="[
            { label: t('dashboard.read'), values: series[0], color: 'var(--chart-2)' },
            {
              label: t('dashboard.written'),
              values: series[1],
              color: 'var(--chart-4)',
              dashed: true,
            },
          ]"
          :base="1024"
          :format="formatRate"
          :height="120"
          :legend="false"
        />
      </PanelSection>
    </div>
  </div>
</template>
