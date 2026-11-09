<script setup lang="ts">
import { FolderInput, Play, RotateCcw, Search, Trash2 } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerBackupGroup, VolumeBackup } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { Input } from '@/shared/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import {
  deleteBackupPlan,
  removeBackup,
  resumeBackupPlan,
  useBackupGroups,
  useRefreshDocker,
  useVolumes,
} from '../api'
import { backupWorkRuns, groupMatches, scheduleText, volumeJob } from '../lib/volumes'
import ImportBackupsDialog from './ImportBackupsDialog.vue'
import RestoreDialog from './RestoreDialog.vue'

const { t } = useI18n()
const jobs = useJobsStore()
const notifications = useNotifications()
const refresh = useRefreshDocker()
/** A backup, restore or import runs: no backup is deleted meanwhile, and the list is asked again. */
const working = computed(() => backupWorkRuns(jobs.list))
const { data: groups, error, isPending } = useBackupGroups(working)
const { data: volumes } = useVolumes()
const filter = ref('')
const restoring = ref<VolumeBackup | null>(null)
const removing = ref<VolumeBackup | null>(null)
const ending = ref<DockerBackupGroup | null>(null)
const importing = ref(false)
const failure = ref<string | null>(null)

const shown = computed(() => (groups.value ?? []).filter((g) => groupMatches(g, filter.value)))

/** What the group's header says of its plan. */
function planText(group: DockerBackupGroup): string {
  if (!group.plan) return t('docker.backups.noPlan')
  const schedule = scheduleText(group.plan, t)
  return group.plan.paused
    ? schedule
    : `${schedule} · ${t('docker.volumes.next', { at: formatDateTime(group.plan.next_at * 1000) })}`
}

async function act(task: () => Promise<unknown>, done: string): Promise<void> {
  failure.value = null
  try {
    await task()
    notifications.notify('success', done)
    await refresh()
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

function resume(group: DockerBackupGroup): Promise<void> {
  return act(() => resumeBackupPlan(group.volume), t('docker.backups.resumed'))
}

function endPlan(): void {
  const group = ending.value
  ending.value = null
  if (group) void act(() => deleteBackupPlan(group.volume), t('docker.backups.planDeleted'))
}

function remove(): void {
  const backup = removing.value
  removing.value = null
  if (backup) void act(() => removeBackup(backup.id), t('docker.backups.removed'))
}

const variants = { running: 'info', succeeded: 'success', failed: 'destructive' } as const
</script>

<!-- Every recorded backup by volume, also of volumes deleted since: restored, deleted and imported from here. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <Button variant="secondary" @click="importing = true">
        <FolderInput />
        {{ t('docker.backups.import') }}
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
      {{ t('docker.backups.none') }}
    </p>
    <section
      v-for="group in shown"
      :key="group.volume"
      class="overflow-hidden rounded-xl border bg-card"
      :aria-label="group.volume"
    >
      <header class="grid gap-1 border-b bg-muted/50 px-4 py-3">
        <div class="flex flex-wrap items-center gap-2">
          <h3 class="font-medium break-all">{{ group.volume }}</h3>
          <Badge
            v-if="!group.exists"
            variant="destructive"
            :title="t('docker.backups.deletedHint')"
            >{{ t('docker.backups.deleted') }}</Badge
          >
          <span class="text-sm text-muted-foreground">{{ planText(group) }}</span>
          <Badge v-if="group.plan?.paused" variant="warning">{{
            t('docker.backups.paused')
          }}</Badge>
          <span v-if="group.plan?.paused" class="ml-auto flex flex-wrap gap-2">
            <Button size="sm" variant="outline" :disabled="!group.exists" @click="resume(group)">
              <Play />
              {{ t('docker.backups.resume') }}
            </Button>
            <Button size="sm" variant="ghost" class="text-destructive" @click="ending = group">{{
              t('docker.backups.deletePlan')
            }}</Button>
          </span>
        </div>
        <p v-if="group.plan?.paused" class="text-xs text-muted-foreground">
          {{ t('docker.backups.pausedHint') }}
          {{ group.exists ? t('docker.backups.pausedBack') : t('docker.backups.pausedGone') }}
        </p>
      </header>
      <p v-if="!group.backups.length" class="px-4 py-6 text-center text-sm text-muted-foreground">
        {{ t('docker.volumes.list.none') }}
      </p>
      <Table v-else>
        <TableHeader>
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('docker.volumes.list.when') }}</TableHead>
            <TableHead>{{ t('docker.volumes.list.size') }}</TableHead>
            <TableHead>{{ t('docker.volumes.list.result') }}</TableHead>
            <TableHead class="hidden @3xl:table-cell">{{ t('docker.volumes.list.by') }}</TableHead>
            <TableHead class="w-24"
              ><span class="sr-only">{{ t('people.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="backup in group.backups" :key="backup.id">
            <TableCell>
              <span class="grid gap-0.5">
                <span class="whitespace-nowrap">{{
                  formatDateTime(backup.started_at * 1000)
                }}</span>
                <span
                  v-if="backup.archive"
                  class="font-mono text-xs break-all text-muted-foreground"
                  >{{ backup.archive }}</span
                >
              </span>
            </TableCell>
            <TableCell class="whitespace-nowrap tabular-nums">{{
              backup.archive_bytes == null ? '–' : formatBytes(backup.archive_bytes)
            }}</TableCell>
            <TableCell>
              <span class="grid justify-items-start gap-0.5">
                <span class="flex flex-wrap gap-1">
                  <Badge :variant="variants[backup.state]">{{
                    t(`docker.volumes.list.states.${backup.state}`)
                  }}</Badge>
                  <Badge
                    v-if="backup.imported"
                    variant="outline"
                    :title="t('docker.backups.importedHint')"
                    >{{ t('docker.backups.imported') }}</Badge
                  >
                </span>
                <span v-if="backup.error" class="text-xs break-words text-destructive">{{
                  backup.error
                }}</span>
              </span>
            </TableCell>
            <TableCell class="hidden text-muted-foreground @3xl:table-cell">{{
              backup.started_by ??
              (backup.imported ? t('docker.backups.imported') : t('docker.volumes.list.schedule'))
            }}</TableCell>
            <TableCell>
              <span class="flex justify-end">
                <Button
                  variant="ghost"
                  size="icon"
                  :disabled="
                    backup.state !== 'succeeded' || volumeJob(jobs.list, group.volume) !== undefined
                  "
                  :title="t('docker.volumes.list.restore')"
                  :aria-label="t('docker.volumes.list.restore')"
                  @click="restoring = backup"
                >
                  <RotateCcw />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="text-destructive"
                  :disabled="backup.state === 'running' || working"
                  :title="t('docker.backups.remove')"
                  :aria-label="t('docker.backups.remove')"
                  @click="removing = backup"
                >
                  <Trash2 />
                </Button>
              </span>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </section>
    <RestoreDialog
      :open="restoring !== null"
      :backup="restoring"
      :volumes="volumes ?? []"
      @update:open="(value: boolean) => !value && (restoring = null)"
    />
    <ImportBackupsDialog v-model:open="importing" />
    <ConfirmDialog
      :open="removing !== null"
      :title="t('docker.backups.removeTitle', { name: removing?.volume ?? '' })"
      :description="
        removing?.archive
          ? t('docker.backups.removeText', {
              at: formatDateTime((removing?.started_at ?? 0) * 1000),
            })
          : t('docker.backups.removeFailedText')
      "
      :confirm-label="t('docker.actions.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (removing = null)"
      @confirm="remove"
    />
    <ConfirmDialog
      :open="ending !== null"
      :title="t('docker.backups.deletePlanTitle', { name: ending?.volume ?? '' })"
      :description="t('docker.backups.deletePlanText')"
      :confirm-label="t('docker.backups.deletePlan')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (ending = null)"
      @confirm="endPlan"
    />
  </div>
</template>
