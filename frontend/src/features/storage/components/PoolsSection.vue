<script setup lang="ts">
import {
  CircleStop,
  Database,
  Plus,
  RefreshCw,
  Replace,
  ShieldCheck,
  Trash2,
} from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Pool, PoolDevice, VdevState } from '@/shared/api/types'
import { useJobsStore } from '@/shared/jobs/store'
import { problemMessage } from '@/shared/i18n'
import {
  formatBytes,
  formatDateTime,
  formatDuration,
  formatPercent,
  formatRate,
} from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge, type BadgeVariants } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { Meter } from '@/shared/ui/meter'

import { scrubPool, stopScrub, useArrays, useDisks, usePools, useRefreshStorage } from '../api'
import { formatPoolLayout } from '../format'
import ArraysSection from './ArraysSection.vue'
import CreatePoolDialog from './CreatePoolDialog.vue'
import DestroyPoolDialog from './DestroyPoolDialog.vue'
import ReplaceDiskDialog from './ReplaceDiskDialog.vue'

/** Whether the signed-in account may change pools (`storage.manage`). */
const props = defineProps<{ canManage: boolean }>()

const { t } = useI18n()
const jobs = useJobsStore()
const refresh = useRefreshStorage()
const { data: pools, error, isPending } = usePools()
const { data: arrays } = useArrays()
const { data: disks } = useDisks()

const creating = ref(false)
const destroying = ref<Pool | null>(null)
const replacing = ref<{ pool: string; device: PoolDevice } | null>(null)
const actionError = ref<string | null>(null)

const managed = (pool: Pool) => props.canManage && pool.volume !== null && pool.volume !== undefined

// Storage jobs change pools and disks: reload when one finishes.
watch(
  () =>
    jobs.list.filter((job) => job.kind.startsWith('storage.') && job.state !== 'running').length,
  () => void refresh(),
)

async function scrub(pool: Pool, action: (name: string) => Promise<void>): Promise<void> {
  actionError.value = null
  try {
    await action(pool.name)
    await refresh()
  } catch (failure) {
    actionError.value = problemMessage(failure)
  }
}

/** Only a running scrub can be stopped; a rebuild must finish. */
const stoppable = (pool: Pool) => pool.scan?.kind === 'scrub' && pool.scan.state === 'running'

const models = computed(
  () => new Map(disks.value?.map((disk) => [disk.name, disk.model ?? disk.name]) ?? []),
)

const STATE: Record<VdevState, BadgeVariants['variant']> = {
  online: 'success',
  degraded: 'warning',
  faulted: 'destructive',
  offline: 'secondary',
  removed: 'destructive',
  unavailable: 'destructive',
  suspended: 'destructive',
  unknown: 'outline',
}

function usage(pool: Pool): number {
  const used = pool.used_bytes ?? 0
  const total = used + (pool.available_bytes ?? 0)
  return total > 0 ? used / total : 0
}

function scanLine(pool: Pool): string | null {
  const scan = pool.scan
  if (!scan) return null
  if (scan.state === 'running') {
    const line = t(`storage.scan.running.${scan.kind}`, { percent: formatPercent(scan.percent) })
    if (scan.paused) return `${line} · ${t('storage.scan.paused')}`
    if (scan.bytes_per_second == null || scan.seconds_left == null) return line
    const pace = t('storage.scan.pace', {
      rate: formatRate(scan.bytes_per_second),
      left: formatDuration(scan.seconds_left),
    })
    return `${line} · ${pace}`
  }
  const at = scan.finished_at ? formatDateTime(scan.finished_at * 1000) : '–'
  if (scan.state === 'canceled') return t(`storage.scan.canceled.${scan.kind}`, { at })
  return t(`storage.scan.finished.${scan.kind}`, { at, count: scan.errors }, scan.errors)
}
</script>

<template>
  <div class="grid gap-3">
    <div v-if="canManage" class="flex justify-end gap-2">
      <Button variant="outline" size="sm" @click="refresh()">
        <RefreshCw />
        {{ t('storage.refresh') }}
      </Button>
      <Button size="sm" @click="creating = true">
        <Plus />
        {{ t('storage.create.button') }}
      </Button>
    </div>
    <Alert v-if="actionError" variant="destructive">{{ actionError }}</Alert>
    <Alert v-if="error" variant="destructive">
      {{ t('storage.poolsUnavailable') }} {{ problemMessage(error) }}
    </Alert>
    <p v-else-if="!isPending && !pools?.length" class="text-sm text-muted-foreground">
      {{ t('storage.noPools') }}
    </p>
    <section
      v-for="pool in pools"
      :key="pool.guid"
      class="grid gap-3 rounded-xl border bg-card p-4"
    >
      <header class="flex flex-wrap items-center gap-3">
        <Database class="size-5 shrink-0 text-primary" />
        <div class="min-w-0 flex-1">
          <h3 class="font-medium">{{ pool.name }} · {{ formatPoolLayout(pool) }}</h3>
          <p class="text-xs text-muted-foreground">
            {{ pool.mount_point ?? t('storage.notMounted') }} ·
            {{ t('storage.redundancy', pool.redundancy) }}
          </p>
        </div>
        <Badge v-if="pool.volume === null || pool.volume === undefined" variant="outline">
          {{ t('storage.notManaged') }}
        </Badge>
        <Badge :variant="STATE[pool.state]">{{ t(`storage.vdevState.${pool.state}`) }}</Badge>
      </header>

      <div v-if="pool.used_bytes !== null && pool.used_bytes !== undefined" class="grid gap-1.5">
        <Meter :value="usage(pool)" :label="t('storage.usedSpace')" />
        <p class="text-xs text-muted-foreground">
          {{
            t('storage.spaceUsed', {
              used: formatBytes(pool.used_bytes),
              size: formatBytes(pool.used_bytes + (pool.available_bytes ?? 0)),
              available: formatBytes(pool.available_bytes ?? 0),
            })
          }}
        </p>
      </div>

      <div v-if="pool.scan" class="grid gap-1.5">
        <p class="text-sm">{{ scanLine(pool) }}</p>
        <Meter
          v-if="pool.scan.state === 'running'"
          :value="pool.scan.percent / 100"
          :label="t(`storage.scan.kind.${pool.scan.kind}`)"
          kind="progress"
        />
      </div>

      <Alert v-if="pool.data_errors > 0" variant="destructive">
        {{ t('storage.dataErrors', pool.data_errors) }}
      </Alert>

      <ul class="grid gap-1.5" :aria-label="t('storage.devices')">
        <li
          v-for="device in pool.devices"
          :key="device.guid"
          class="flex flex-wrap items-center gap-2 rounded-lg border px-3 py-1.5 text-sm"
        >
          <span class="font-medium">{{ device.disk ?? t('storage.missingDisk') }}</span>
          <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground" :title="device.name">
            {{ (device.disk && models.get(device.disk)) ?? device.name }}
          </span>
          <span
            v-if="device.read_errors + device.write_errors + device.checksum_errors > 0"
            class="text-xs text-destructive tabular-nums"
          >
            {{
              t('storage.deviceErrors', {
                read: device.read_errors,
                write: device.write_errors,
                checksum: device.checksum_errors,
              })
            }}
          </span>
          <Badge v-if="device.replacing" variant="default">
            {{ t(`storage.replacing.${device.replacing}`) }}
          </Badge>
          <Badge :variant="STATE[device.state]">{{ t(`storage.vdevState.${device.state}`) }}</Badge>
          <Button
            v-if="managed(pool) && !device.replacing && pool.layout !== 'single'"
            variant="ghost"
            size="icon"
            :title="t('storage.replaceDisk.button')"
            :aria-label="t('storage.replaceDisk.button')"
            @click="replacing = { pool: pool.name, device }"
          >
            <Replace />
          </Button>
        </li>
      </ul>

      <div v-if="managed(pool)" class="flex flex-wrap justify-end gap-2">
        <Button v-if="stoppable(pool)" variant="outline" size="sm" @click="scrub(pool, stopScrub)">
          <CircleStop />
          {{ t('storage.stopScrub') }}
        </Button>
        <Button
          v-else
          variant="outline"
          size="sm"
          :disabled="pool.scan?.state === 'running'"
          @click="scrub(pool, scrubPool)"
        >
          <ShieldCheck />
          {{ t('storage.scrubNow') }}
        </Button>
        <Button variant="outline" size="sm" @click="destroying = pool">
          <Trash2 />
          {{ t('storage.destroy.button') }}
        </Button>
      </div>
    </section>

    <CreatePoolDialog v-model:open="creating" />
    <DestroyPoolDialog
      v-if="destroying"
      :open="true"
      :pool="destroying"
      @update:open="(open) => !open && (destroying = null)"
    />
    <ReplaceDiskDialog
      v-if="replacing"
      :open="true"
      :pool="replacing.pool"
      :device="replacing.device"
      @update:open="(open) => !open && (replacing = null)"
    />

    <template v-if="arrays?.length">
      <h3 class="mt-2 text-sm font-medium text-muted-foreground">{{ t('storage.otherArrays') }}</h3>
      <ArraysSection />
    </template>
  </div>
</template>
