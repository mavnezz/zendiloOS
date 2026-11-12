<script setup lang="ts">
import { Archive, ArchiveRestore, Trash2 } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VmBackup, VmPlugin } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { ActionMenu, type Action } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

import { useBackupActions, useVmBackups, useVms } from '../api'
import { WORK_KINDS, consistencyVariant } from '../lib/backups'
import { formatMemory } from '../lib/vms'
import RestoreBackupDialog from './RestoreBackupDialog.vue'

/** Every VM's backups: restored as a new VM or over the old one, and deleted. */
defineProps<{ plugin: VmPlugin }>()

const { t } = useI18n()
const jobs = useJobsStore()
const actions = useBackupActions()
const busy = computed(() =>
  jobs.list.some((job) => job.state === 'running' && WORK_KINDS.includes(job.kind)),
)
const {
  data: backups,
  error,
  isPending,
} = useVmBackups(
  () => true,
  () => busy.value,
)
const { data: vms } = useVms()

const restoring = ref<VmBackup | null>(null)
const deleting = ref<VmBackup | null>(null)
const failure = ref<string | null>(null)

const vmOf = (backup: VmBackup) =>
  backup.vm_exists ? ((vms.value ?? []).find((vm) => vm.id === backup.vm) ?? null) : null
const taken = computed(() => (vms.value ?? []).map((vm) => vm.name))
const when = (backup: VmBackup | null) => (backup ? formatDateTime(backup.taken_at * 1000) : '')

const menu = computed<Action[]>(() => [
  { id: 'restore', label: t('vms.backups.restore'), icon: ArchiveRestore },
  { id: 'delete', label: t('vms.backups.delete'), icon: Trash2, destructive: true },
])

function select(backup: VmBackup, id: string): void {
  if (id === 'restore') restoring.value = backup
  else deleting.value = backup
}

async function remove(): Promise<void> {
  const backup = deleting.value
  if (!backup) return
  failure.value = null
  try {
    await actions.remove(backup.id)
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}
</script>

<!-- The VMs' backups, in shares and on USB drives. -->
<template>
  <div class="grid content-start gap-6">
    <header class="grid gap-0.5">
      <h2 class="text-xl font-semibold">{{ t('vms.sections.backups') }}</h2>
      <p class="text-sm text-muted-foreground">{{ t('vms.backups.description') }}</p>
    </header>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive" data-testid="vm-backups-failure">{{
      failure
    }}</Alert>

    <div v-if="isPending" class="h-32 animate-pulse rounded-xl bg-muted" />
    <div
      v-else-if="!backups?.length"
      class="grid justify-items-center gap-3 rounded-xl border border-dashed p-10 text-center"
      data-testid="vm-backups-empty"
    >
      <Archive class="size-10 text-muted-foreground" />
      <p class="font-medium">{{ t('vms.backups.empty') }}</p>
      <p class="max-w-md text-sm text-muted-foreground">{{ t('vms.backups.emptyHint') }}</p>
    </div>
    <ul v-else class="grid gap-2">
      <li
        v-for="backup in backups"
        :key="backup.id"
        :data-backup="backup.id"
        class="flex flex-wrap items-center gap-3 rounded-xl border bg-card p-3"
      >
        <span
          class="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground"
          aria-hidden="true"
        >
          <Archive class="size-5" />
        </span>
        <div class="grid min-w-0 flex-1 gap-0.5">
          <span class="flex flex-wrap items-center gap-2 font-medium">
            {{ backup.vm_name }}
            <Badge v-if="!backup.vm_exists" variant="secondary">{{
              t('vms.backups.vmGone')
            }}</Badge>
          </span>
          <span class="truncate text-xs text-muted-foreground" :title="backup.folder">
            {{ t('vms.backups.where', { place: backup.place, folder: backup.folder }) }}
          </span>
          <span class="text-xs text-muted-foreground">
            {{ when(backup) }} · {{ t(`vms.os.${backup.os}`) }} · {{ t('vms.cpus', backup.cpus) }} ·
            {{ formatMemory(backup.memory_mib) }} ·
            {{ backup.disks_gib.map((gib) => `${gib} GiB`).join(', ') }}
          </span>
        </div>
        <span class="text-sm tabular-nums">{{ formatBytes(backup.bytes) }}</span>
        <span class="flex flex-wrap gap-1">
          <Badge
            :variant="consistencyVariant(backup.consistency)"
            :title="t(`vms.snapshots.consistencyHint.${backup.consistency}`)"
            >{{ t(`vms.snapshots.consistency.${backup.consistency}`) }}</Badge
          >
          <Badge v-if="backup.tpm" variant="outline">{{ t('vms.security.tpm') }}</Badge>
          <Badge v-if="backup.secure_boot" variant="outline">{{
            t('vms.security.secureBoot')
          }}</Badge>
        </span>
        <ActionMenu
          :actions="menu"
          :label="t('vms.backups.more', { name: backup.vm_name })"
          @select="(id: string) => select(backup, id)"
        />
      </li>
    </ul>

    <RestoreBackupDialog
      v-if="restoring"
      :backup="restoring"
      :plugin="plugin"
      :vm="vmOf(restoring)"
      :taken="taken"
      :open="Boolean(restoring)"
      @update:open="(open: boolean) => !open && (restoring = null)"
    />
    <ConfirmDialog
      :open="Boolean(deleting)"
      :title="t('vms.backups.deleteTitle', { name: deleting?.vm_name ?? '', when: when(deleting) })"
      :description="t('vms.backups.deleteText', { place: deleting?.place ?? '' })"
      :confirm-label="t('vms.backups.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (deleting = null)"
      @confirm="remove"
    />
  </div>
</template>
