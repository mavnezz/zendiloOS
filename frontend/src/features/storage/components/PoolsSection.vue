<script setup lang="ts">
import {
  CircleStop,
  Database,
  LoaderCircle,
  Plus,
  Replace,
  ShieldCheck,
  Trash2,
} from 'lucide-vue-next'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Job, Pool, PoolDevice, VdevState } from '@/shared/api/types'
import { jobTitle, useJobsStore } from '@/shared/jobs/store'
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
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { scrubPool, stopScrub, useArrays, useDisks, usePools, useRefreshStorage } from '../api'
import { formatPoolLayout } from '../format'
import AbsentPools from './AbsentPools.vue'
import ArraysSection from './ArraysSection.vue'
import CreatePoolDialog from './CreatePoolDialog.vue'
import DestroyPoolDialog from './DestroyPoolDialog.vue'
import ReplaceDiskDialog from './ReplaceDiskDialog.vue'

/** Whether the signed-in account may change pools (`storage.manage`). */
const props = defineProps<{ canManage: boolean }>()

const i18n = useI18n()
const { t } = i18n
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

/** Pool jobs under way: creating takes a minute or more, with nothing to show yet. */
const POOL_JOBS = [
  'storage.create_pool',
  'storage.destroy_pool',
  'storage.replace_disk',
  'storage.import_pool',
]
const underway = computed(() => jobs.running.filter((job) => POOL_JOBS.includes(job.kind)))
const now = ref(Date.now() / 1000)
const ticking = setInterval(() => (now.value = Date.now() / 1000), 1000)
onBeforeUnmount(() => clearInterval(ticking))
const runningFor = (since: number) => formatDuration(Math.max(0, Math.round(now.value - since)))

/** The step the agent reported, e.g. "Preparing the disks (50 %)", else a general word. */
function stepOf(job: Job): string {
  const key = `storage.steps.${job.step ?? ''}`
  if (!job.step || !i18n.te(key)) return t('storage.underway')
  const share = job.progress
  return share !== null && share !== undefined && share > 0 && share < 1
    ? `${t(key)} (${formatPercent(share * 100)})`
    : t(key)
}

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
  <div class="grid gap-4">
    <div class="flex items-center gap-2">
      <p class="flex-1 text-xs text-muted-foreground">
        {{ t('storage.poolCount', { count: pools?.length ?? 0 }, pools?.length ?? 0) }}
      </p>
      <Button v-if="canManage" size="sm" @click="creating = true">
        <Plus />
        {{ t('storage.create.button') }}
      </Button>
    </div>
    <Alert v-if="actionError" variant="destructive">{{ actionError }}</Alert>
    <Alert v-if="error" variant="destructive">
      {{ t('storage.poolsUnavailable') }} {{ problemMessage(error) }}
    </Alert>
    <AbsentPools v-if="canManage" />
    <p
      v-else-if="!isPending && !pools?.length && !underway.length"
      class="text-sm text-muted-foreground"
    >
      {{ t('storage.noPools') }}
    </p>
    <section
      v-for="job in underway"
      :key="job.id"
      class="flex items-center gap-3 rounded-lg border border-dashed p-4 text-sm"
      aria-live="polite"
    >
      <LoaderCircle class="size-4 shrink-0 animate-spin text-primary" />
      <span class="font-medium">{{ jobTitle(job) }}</span>
      <span class="flex-1 text-muted-foreground">{{ stepOf(job) }}</span>
      <span class="text-xs text-muted-foreground tabular-nums">{{
        runningFor(job.created_at)
      }}</span>
    </section>
    <section v-for="pool in pools" :key="pool.guid" class="grid gap-3 rounded-lg border p-4">
      <header class="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Database class="size-4 shrink-0 text-muted-foreground" />
        <h3 class="font-mono text-sm font-semibold">{{ pool.name }}</h3>
        <span class="text-xs text-muted-foreground">
          {{ formatPoolLayout(pool) }} · {{ pool.mount_point ?? t('storage.notMounted') }} ·
          {{ t('storage.redundancy', pool.redundancy) }}
        </span>
        <span class="flex-1" />
        <Badge v-if="pool.volume === null || pool.volume === undefined" variant="outline">
          {{ t('storage.notManaged') }}
        </Badge>
        <Badge :variant="STATE[pool.state]">{{ t(`storage.vdevState.${pool.state}`) }}</Badge>
        <template v-if="managed(pool)">
          <Button v-if="stoppable(pool)" variant="ghost" size="sm" @click="scrub(pool, stopScrub)">
            <CircleStop />
            {{ t('storage.stopScrub') }}
          </Button>
          <Button
            v-else
            variant="ghost"
            size="sm"
            :disabled="pool.scan?.state === 'running'"
            @click="scrub(pool, scrubPool)"
          >
            <ShieldCheck />
            {{ t('storage.scrubNow') }}
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            class="text-muted-foreground hover:text-destructive"
            :title="t('storage.destroy.button')"
            :aria-label="t('storage.destroy.button')"
            @click="destroying = pool"
          >
            <Trash2 />
          </Button>
        </template>
      </header>

      <div
        v-if="pool.used_bytes !== null && pool.used_bytes !== undefined"
        class="grid grid-cols-[1fr_auto] items-center gap-x-4 gap-y-1"
      >
        <Meter :value="usage(pool)" :label="t('storage.usedSpace')" />
        <span class="text-xs font-medium tabular-nums">{{ formatPercent(usage(pool) * 100) }}</span>
        <p class="text-xs text-muted-foreground tabular-nums">
          {{
            t('storage.spaceUsed', {
              used: formatBytes(pool.used_bytes),
              size: formatBytes(pool.used_bytes + (pool.available_bytes ?? 0)),
              available: formatBytes(pool.available_bytes ?? 0),
            })
          }}
        </p>
      </div>

      <div v-if="pool.scan" class="grid gap-1.5 rounded-md bg-muted/50 px-3 py-2">
        <p class="text-xs">{{ scanLine(pool) }}</p>
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

      <Table :aria-label="t('storage.devices')">
        <TableHeader>
          <TableRow>
            <TableHead>{{ t('storage.columns.device') }}</TableHead>
            <TableHead>{{ t('storage.columns.model') }}</TableHead>
            <TableHead class="hidden @3xl:table-cell">{{ t('storage.columns.member') }}</TableHead>
            <TableHead class="text-right">{{ t('storage.columns.readErrors') }}</TableHead>
            <TableHead class="text-right">{{ t('storage.columns.writeErrors') }}</TableHead>
            <TableHead class="text-right">{{ t('storage.columns.checksumErrors') }}</TableHead>
            <TableHead>{{ t('storage.columns.state') }}</TableHead>
            <TableHead class="w-8" />
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="device in pool.devices" :key="device.guid">
            <TableCell class="font-mono text-xs font-medium">
              {{ device.disk ?? t('storage.missingDisk') }}
            </TableCell>
            <TableCell class="max-w-48 truncate">
              {{ (device.disk && models.get(device.disk)) ?? '–' }}
            </TableCell>
            <TableCell
              class="hidden max-w-72 truncate font-mono text-[11px] text-muted-foreground @3xl:table-cell"
              :title="device.name"
            >
              {{ device.name }}
            </TableCell>
            <TableCell
              v-for="(count, kind) in [
                device.read_errors,
                device.write_errors,
                device.checksum_errors,
              ]"
              :key="kind"
              :class="[
                'text-right tabular-nums',
                count > 0 ? 'font-medium text-destructive' : 'text-muted-foreground',
              ]"
            >
              {{ count }}
            </TableCell>
            <TableCell>
              <span class="flex items-center gap-1">
                <Badge v-if="device.replacing" variant="default">
                  {{ t(`storage.replacing.${device.replacing}`) }}
                </Badge>
                <Badge :variant="STATE[device.state]">{{
                  t(`storage.vdevState.${device.state}`)
                }}</Badge>
              </span>
            </TableCell>
            <TableCell>
              <Button
                v-if="managed(pool) && !device.replacing && pool.layout !== 'single'"
                variant="ghost"
                size="icon-sm"
                :title="t('storage.replaceDisk.button')"
                :aria-label="t('storage.replaceDisk.button')"
                @click="replacing = { pool: pool.name, device }"
              >
                <Replace />
              </Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
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
      <h3 class="mt-2 text-sm font-semibold">{{ t('storage.otherArrays') }}</h3>
      <ArraysSection />
    </template>
  </div>
</template>
