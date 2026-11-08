<script setup lang="ts">
import { CalendarClock, Eraser, HardDriveDownload, History, Search, Trash2 } from 'lucide-vue-next'
import { computed, inject, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerVolume } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { useNotifications } from '@/shared/lib/notifications'
import { type Action, ActionMenu } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import {
  backUpNow,
  usePruneVolumes,
  useRefreshDocker,
  useRemoveVolume,
  useVolumes,
  useVolumeSizes,
} from '../api'
import { OPEN_CONTAINER } from '../lib/navigation'
import { matches, scheduleText, volumeJob } from '../lib/volumes'
import BackupPlanDialog from './BackupPlanDialog.vue'
import VolumeBackupsDialog from './VolumeBackupsDialog.vue'

const i18n = useI18n()
const { t } = i18n
const notifications = useNotifications()
const jobs = useJobsStore()
const { data: volumes, isPending } = useVolumes()
const { data: sizes } = useVolumeSizes()
const remove = useRemoveVolume()
const prune = usePruneVolumes()
const refresh = useRefreshDocker()
const openContainer = inject(OPEN_CONTAINER, () => {})
const filter = ref('')
const removing = ref<DockerVolume | null>(null)
const pruning = ref(false)
const planning = ref<DockerVolume | null>(null)
const listing = ref<DockerVolume | null>(null)
const failure = ref<string | null>(null)

const shown = computed(() => (volumes.value ?? []).filter((v) => matches(v, filter.value)))
const error = computed(() => remove.error.value ?? prune.error.value)
const bySize = computed(() => new Map((sizes.value ?? []).map((size) => [size.name, size])))

function size(volume: DockerVolume): string {
  const counted = bySize.value.get(volume.name)
  if (!counted) return volume.backed_up_here && !sizes.value ? '…' : '–'
  const bytes = formatBytes(counted.bytes)
  return counted.complete ? bytes : t('docker.volumes.atLeast', { size: bytes })
}

/** What the backup column says of a volume. */
function backup(volume: DockerVolume): { text: string; detail?: string; failed?: boolean } {
  const job = volumeJob(jobs.list, volume.name)
  if (job) {
    const key = `docker.volumes.steps.${job.step ?? ''}`
    const step = i18n.te(key) ? t(key) : t('docker.volumes.backingUp')
    const percent = job.progress == null ? '' : ` (${Math.round(job.progress * 100)} %)`
    return { text: `${step}${percent}` }
  }
  if (!volume.backed_up_here) return { text: t('docker.volumes.elsewhere') }
  const last = volume.last_backup
  const failed = last?.state === 'failed'
  if (!volume.plan) {
    return {
      text: t('docker.volumes.noBackup'),
      detail: last?.finished_at
        ? t('docker.volumes.lastAt', { at: formatDateTime(last.finished_at * 1000) })
        : undefined,
      failed,
    }
  }
  return {
    text: scheduleText(volume.plan, t),
    detail: failed
      ? t('docker.volumes.lastFailed')
      : t('docker.volumes.next', { at: formatDateTime(volume.plan.next_at * 1000) }),
    failed,
  }
}

function actions(volume: DockerVolume): Action[] {
  const busy = volumeJob(jobs.list, volume.name) !== undefined
  return [
    {
      id: 'plan',
      label: volume.plan ? t('docker.volumes.editPlan') : t('docker.volumes.setUp'),
      icon: CalendarClock,
      disabled: !volume.backed_up_here,
    },
    {
      id: 'now',
      label: t('docker.volumes.backUpNow'),
      icon: HardDriveDownload,
      disabled: !volume.plan || busy,
    },
    { id: 'backups', label: t('docker.volumes.backups'), icon: History },
    {
      id: 'remove',
      label: t('docker.volumes.remove'),
      icon: Trash2,
      destructive: true,
      disabled: volume.used_by.length > 0 || busy,
    },
  ]
}

async function run(volume: DockerVolume, action: string): Promise<void> {
  failure.value = null
  if (action === 'plan') planning.value = volume
  else if (action === 'backups') listing.value = volume
  else if (action === 'remove') removing.value = volume
  else if (action === 'now') {
    try {
      jobs.update(await backUpNow(volume.name))
      notifications.notify('info', t('docker.volumes.backUpNow'), t('docker.volumes.started'))
      await refresh()
    } catch (problem) {
      failure.value = problemMessage(problem)
    }
  }
}

function clean(): void {
  prune.mutate(undefined, {
    onSuccess: (result) =>
      notifications.notify('success', t('docker.volumes.pruned'), result.text.trim()),
  })
}

function confirmRemove(): void {
  if (removing.value) remove.mutate(removing.value.name)
  removing.value = null
}
</script>

<!-- The engine's volumes, who uses them, and how they are backed up. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <Button variant="secondary" :disabled="prune.isPending.value" @click="pruning = true">
        <Eraser />
        {{ t('docker.volumes.prune') }}
      </Button>
      <div class="relative ml-auto w-full max-w-60">
        <Search class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="filter"
          class="pl-8"
          :placeholder="t('docker.search')"
          :aria-label="t('docker.search')"
        />
      </div>
    </div>
    <Alert v-if="error || failure" variant="destructive">{{
      failure ?? problemMessage(error)
    }}</Alert>
    <div v-if="isPending" class="h-40 animate-pulse rounded-xl bg-muted" />
    <p v-else-if="!shown.length" class="py-16 text-center text-sm text-muted-foreground">
      {{ t('docker.volumes.none') }}
    </p>
    <div v-else class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('docker.volumes.name') }}</TableHead>
            <TableHead class="hidden @3xl:table-cell">{{ t('docker.volumes.driver') }}</TableHead>
            <TableHead>{{ t('docker.volumes.size') }}</TableHead>
            <TableHead class="hidden @3xl:table-cell">{{ t('docker.volumes.created') }}</TableHead>
            <TableHead>{{ t('docker.volumes.usedBy') }}</TableHead>
            <TableHead>{{ t('docker.volumes.backup') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('people.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="volume in shown" :key="volume.name">
            <TableCell>
              <span class="grid gap-0.5">
                <span class="font-medium break-all">{{ volume.name }}</span>
                <span v-if="volume.project" class="text-xs text-muted-foreground">{{
                  volume.project
                }}</span>
              </span>
            </TableCell>
            <TableCell class="hidden text-muted-foreground @3xl:table-cell">{{
              volume.driver
            }}</TableCell>
            <TableCell class="whitespace-nowrap tabular-nums">{{ size(volume) }}</TableCell>
            <TableCell class="hidden text-muted-foreground @3xl:table-cell">{{
              volume.created_at ? formatDateTime(volume.created_at * 1000) : '–'
            }}</TableCell>
            <TableCell>
              <span v-if="volume.used_by.length" class="flex flex-wrap gap-x-2 gap-y-0.5 text-sm">
                <button
                  v-for="user in volume.used_by"
                  :key="user.name"
                  type="button"
                  class="text-left text-primary hover:underline"
                  @click="openContainer(user.name)"
                >
                  {{ user.name
                  }}<span v-if="!user.running" class="text-muted-foreground">
                    ({{ t('docker.volumes.stopped') }})</span
                  >
                </button>
              </span>
              <Badge v-else variant="outline">{{ t('docker.volumes.unused') }}</Badge>
            </TableCell>
            <TableCell>
              <span
                class="grid gap-0.5 text-sm"
                :title="volume.backed_up_here ? undefined : t('docker.volumes.elsewhereHint')"
              >
                <span :class="!volume.backed_up_here && 'text-muted-foreground'">{{
                  backup(volume).text
                }}</span>
                <span
                  v-if="backup(volume).detail"
                  class="text-xs"
                  :class="backup(volume).failed ? 'text-destructive' : 'text-muted-foreground'"
                  >{{ backup(volume).detail }}</span
                >
              </span>
            </TableCell>
            <TableCell>
              <ActionMenu
                :actions="actions(volume)"
                :label="t('docker.actions.more')"
                @select="run(volume, $event)"
              />
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <BackupPlanDialog
      :open="planning !== null"
      :volume="planning"
      @update:open="(open: boolean) => !open && (planning = null)"
    />
    <VolumeBackupsDialog
      :open="listing !== null"
      :volume="listing"
      :volumes="volumes ?? []"
      @update:open="(open: boolean) => !open && (listing = null)"
    />
    <ConfirmDialog
      :open="removing !== null"
      :title="t('docker.volumes.removeTitle', { name: removing?.name })"
      :description="t('docker.volumes.removeText')"
      :confirm-label="t('docker.actions.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (removing = null)"
      @confirm="confirmRemove"
    />
    <ConfirmDialog
      v-model:open="pruning"
      :title="t('docker.volumes.prune')"
      :description="t('docker.volumes.pruneText')"
      :confirm-label="t('docker.volumes.prune')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="clean"
    />
  </div>
</template>
