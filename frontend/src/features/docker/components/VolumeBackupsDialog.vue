<script setup lang="ts">
import { HardDriveDownload, RotateCcw } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerVolume, VolumeBackup } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { backUpNow, useBackups, useRefreshDocker } from '../api'
import { scheduleText, volumeJob } from '../lib/volumes'
import RestoreDialog from './RestoreDialog.vue'

const props = defineProps<{ volume: DockerVolume | null; volumes: DockerVolume[] }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const jobs = useJobsStore()
const notifications = useNotifications()
const refresh = useRefreshDocker()
const name = computed(() => props.volume?.name ?? '')
/** A backup or restore of it runs: its list is asked again meanwhile. */
const working = computed(() => volumeJob(jobs.list, name.value) !== undefined)
const { data: backups, error, isPending } = useBackups(name, working)
const restoring = ref<VolumeBackup | null>(null)
const failure = ref<string | null>(null)
const starting = ref(false)

async function now(): Promise<void> {
  failure.value = null
  starting.value = true
  try {
    jobs.update(await backUpNow(name.value))
    notifications.notify('info', t('docker.volumes.backUpNow'), t('docker.volumes.started'))
    await refresh()
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    starting.value = false
  }
}

const variants = { running: 'info', succeeded: 'success', failed: 'destructive' } as const
</script>

<!-- A volume's backups: those kept and the runs that failed lately; each restored from here. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('docker.volumes.list.title', { name })"
    :description="volume?.plan ? scheduleText(volume.plan, t) : t('docker.volumes.list.noPlan')"
    :close-label="t('desktop.close')"
    wide
  >
    <Alert v-if="error || failure" variant="destructive">{{
      failure ?? problemMessage(error)
    }}</Alert>
    <div v-if="isPending" class="h-24 animate-pulse rounded-xl bg-muted" />
    <p v-else-if="!backups?.length" class="py-8 text-center text-sm text-muted-foreground">
      {{ t('docker.volumes.list.none') }}
    </p>
    <div v-else class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('docker.volumes.list.when') }}</TableHead>
            <TableHead>{{ t('docker.volumes.list.size') }}</TableHead>
            <TableHead>{{ t('docker.volumes.list.result') }}</TableHead>
            <TableHead class="hidden @xl:table-cell">{{ t('docker.volumes.list.by') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('people.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="backup in backups" :key="backup.id">
            <TableCell class="whitespace-nowrap">
              <span class="grid gap-0.5">
                <span>{{ formatDateTime(backup.started_at * 1000) }}</span>
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
              <span class="grid gap-0.5">
                <Badge :variant="variants[backup.state]">{{
                  t(`docker.volumes.list.states.${backup.state}`)
                }}</Badge>
                <span v-if="backup.error" class="text-xs break-words text-destructive">{{
                  backup.error
                }}</span>
              </span>
            </TableCell>
            <TableCell class="hidden text-muted-foreground @xl:table-cell">{{
              backup.started_by ?? t('docker.volumes.list.schedule')
            }}</TableCell>
            <TableCell>
              <Button
                variant="ghost"
                size="icon"
                :disabled="backup.state !== 'succeeded' || working"
                :title="t('docker.volumes.list.restore')"
                :aria-label="t('docker.volumes.list.restore')"
                @click="restoring = backup"
              >
                <RotateCcw />
              </Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <div v-if="volume?.plan" class="flex justify-end">
      <Button :disabled="working || starting" @click="now">
        <HardDriveDownload />
        {{ t('docker.volumes.backUpNow') }}
      </Button>
    </div>
    <RestoreDialog
      :open="restoring !== null"
      :backup="restoring"
      :volumes="volumes"
      @update:open="(value: boolean) => !value && (restoring = null)"
      @started="open = false"
    />
  </ModalDialog>
</template>
