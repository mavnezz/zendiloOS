<script setup lang="ts">
import { Clock, Cpu, Database, ExternalLink, Pencil, Server } from 'lucide-vue-next'
import { computed, onScopeDispose, ref, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { live } from '@/shared/api/live'
import type { Metrics, Pool } from '@/shared/api/types'
import { formatBytes, formatDateTime, formatDuration, formatPercent } from '@/shared/lib/format'
import { PROJECT_URL } from '@/shared/lib/star-prompt'
import { Button } from '@/shared/ui/button'
import { Meter } from '@/shared/ui/meter'
import { DefinitionList, PageCard, PageHeader } from '@/shared/ui/page'

import { useHostName, useSystemInfo } from '../api'
import HostNameDialog from './HostNameDialog.vue'

/** The pools, if the account may see storage; `canManage` renames the machine. */
const props = defineProps<{ pools?: Pool[]; canManage?: boolean }>()

const { t } = useI18n()
const { data: info } = useSystemInfo()
const { data: hostName } = useHostName()
/** As the system keeps it now; the information's is the one zendiloOS started with. */
const name = computed(() => hostName.value?.name || info.value?.hostname)
const renaming = ref(false)
const latest = shallowRef<Metrics | null>(null)
onScopeDispose(live.subscribe('metrics', (sample) => (latest.value = sample)))

const uptime = computed(() => latest.value?.uptime_seconds ?? info.value?.uptime_seconds)
const since = computed(() =>
  uptime.value === undefined ? null : formatDateTime(Date.now() - uptime.value * 1000),
)
const load = computed(() => latest.value?.load_average.map((value) => value.toFixed(2)).join(' · '))
const machine = computed(
  () => [info.value?.vendor, info.value?.product].filter(Boolean).join(' ') || null,
)

const device = computed(() => {
  const value = info.value
  if (!value) return []
  return [
    { label: t('device.os'), value: value.os_name ?? 'Linux' },
    { label: t('device.kernel'), value: `${value.kernel_release} (${value.architecture})` },
    ...(machine.value ? [{ label: t('device.machine'), value: machine.value }] : []),
  ]
})

const hardware = computed(() => {
  const value = info.value
  if (!value) return []
  const memory = latest.value?.memory
  return [
    { label: t('device.processor'), value: value.cpu_model ?? value.architecture },
    ...(value.cpu_cores ? [{ label: t('device.cores'), value: String(value.cpu_cores) }] : []),
    { label: t('device.threads'), value: String(value.cpu_threads) },
    { label: t('device.memory'), value: formatBytes(value.memory_total_bytes) },
    ...(memory
      ? [
          {
            label: t('device.memoryUsed'),
            value: t('device.ofTotal', {
              used: formatBytes(memory.used_bytes),
              total: formatBytes(memory.total_bytes),
            }),
          },
        ]
      : []),
    ...(value.swap_total_bytes > 0
      ? [{ label: t('device.swap'), value: formatBytes(value.swap_total_bytes) }]
      : []),
    ...(value.board ? [{ label: t('device.board'), value: value.board }] : []),
    ...(value.firmware
      ? [
          {
            label: t('device.firmware'),
            value: `${value.firmware} · ${value.uefi ? 'UEFI' : 'BIOS'}`,
          },
        ]
      : []),
  ]
})

const pools = computed(() =>
  (props.pools ?? []).filter((pool) => pool.used_bytes != null && pool.available_bytes != null),
)
const share = (pool: Pool) => {
  const total = (pool.used_bytes ?? 0) + (pool.available_bytes ?? 0)
  return total > 0 ? (pool.used_bytes ?? 0) / total : 0
}
</script>

<!--
  Who this machine is, what it is made of, and how long it has been running.
  The cards sit side by side as the window allows: one column, then device
  and uptime beside each other above hardware and storage, then the short
  device and uptime stacked beside the taller hardware and storage.
-->
<template>
  <div>
    <PageHeader :title="t('device.title')" :description="t('device.description')" />
    <div class="grid gap-4 @2xl:grid-cols-2 @6xl:grid-cols-3" data-testid="device-cards">
      <PageCard
        :title="t('device.device')"
        :icon="Server"
        class="@6xl:col-start-1 @6xl:row-start-1"
      >
        <div v-if="info" class="grid gap-4">
          <div>
            <div class="flex items-center gap-2">
              <p class="text-2xl font-semibold tracking-tight" data-testid="host-name">
                {{ name }}
              </p>
              <Button
                v-if="canManage && hostName"
                variant="ghost"
                size="icon"
                :aria-label="t('hostName.rename')"
                :title="t('hostName.rename')"
                @click="renaming = true"
              >
                <Pencil class="size-4" />
              </Button>
            </div>
            <p class="text-sm text-muted-foreground">zendiloOS {{ info.version }}</p>
            <a
              :href="PROJECT_URL"
              target="_blank"
              rel="noopener noreferrer"
              class="mt-1 inline-flex items-center gap-1 text-sm text-primary hover:underline"
            >
              {{ t('device.github') }}
              <ExternalLink class="size-3.5" aria-hidden="true" />
            </a>
          </div>
          <DefinitionList :items="device" />
        </div>
      </PageCard>

      <PageCard :title="t('device.uptime')" :icon="Clock" class="@6xl:col-start-1 @6xl:row-start-2">
        <div v-if="uptime !== undefined" class="grid gap-4">
          <div>
            <p class="text-2xl font-semibold tracking-tight">{{ formatDuration(uptime) }}</p>
            <p class="text-sm text-muted-foreground">{{ t('device.since', { time: since }) }}</p>
          </div>
          <DefinitionList v-if="load" :items="[{ label: t('device.load'), value: load }]" />
        </div>
      </PageCard>

      <PageCard
        :title="t('device.hardware')"
        :icon="Cpu"
        class="@6xl:col-start-2 @6xl:row-span-2 @6xl:row-start-1"
      >
        <DefinitionList :items="hardware" />
      </PageCard>

      <PageCard
        v-if="pools.length"
        :title="t('device.storage')"
        :icon="Database"
        class="self-start @6xl:col-start-3 @6xl:row-span-2 @6xl:row-start-1"
      >
        <div class="grid gap-4">
          <div v-for="pool in pools" :key="pool.guid" class="grid gap-1.5">
            <div class="flex items-baseline justify-between gap-2">
              <span class="font-medium">{{ pool.name }}</span>
              <span class="text-muted-foreground tabular-nums">
                {{
                  t('device.poolUse', {
                    used: formatBytes(pool.used_bytes ?? 0),
                    total: formatBytes((pool.used_bytes ?? 0) + (pool.available_bytes ?? 0)),
                    percent: formatPercent(share(pool) * 100),
                  })
                }}
              </span>
            </div>
            <Meter :value="share(pool)" :label="pool.name" />
          </div>
        </div>
      </PageCard>
    </div>
    <HostNameDialog
      v-if="hostName"
      v-model:open="renaming"
      :name="hostName.name"
      :max-length="hostName.max_length"
    />
  </div>
</template>
