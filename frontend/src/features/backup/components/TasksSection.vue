<script setup lang="ts">
import { ArchiveRestore, Plus } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BackupPlugin, BackupTask } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

import { checkServer, useBackupActions } from '../api'
import { useNames } from '../names'
import TaskCard from './TaskCard.vue'
import TaskDialog from './TaskDialog.vue'

const props = defineProps<{ tasks: BackupTask[]; plugin: BackupPlugin }>()
const emit = defineEmits<{ restore: [task: number]; log: [task: number] }>()

const { t } = useI18n()
const actions = useBackupActions()
const jobs = useJobsStore()
const { shareNames, driveLabels, accountNames } = useNames()

const creating = ref(false)
const editing = ref<BackupTask | null>(null)
const editOpen = computed({
  get: () => editing.value !== null,
  set: (open) => {
    if (!open) editing.value = null
  },
})
const deleting = ref<BackupTask | null>(null)
const deleteOpen = computed({
  get: () => deleting.value !== null,
  set: (open) => {
    if (!open) deleting.value = null
  },
})
const busy = ref<number | null>(null)
const failure = ref<string | null>(null)
const notice = ref<string | null>(null)
const names = computed(() => props.tasks.map((task) => task.name))

async function act(task: BackupTask, work: () => Promise<unknown>): Promise<void> {
  busy.value = task.id
  failure.value = null
  notice.value = null
  try {
    await work()
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = null
  }
}

const run = (task: BackupTask) => act(task, async () => jobs.update(await actions.run(task.id)))
const stop = (task: BackupTask) => act(task, () => actions.stop(task.id))

async function check(task: BackupTask): Promise<void> {
  const place = [task.target, task.source].find((place) => place.kind === 'server')
  if (!task.server || place?.kind !== 'server') return
  const server = task.server
  await act(task, async () => {
    const entries = await checkServer(server, place.path, null, task.id)
    notice.value = entries.length
      ? t('backup.server.checkOk', entries.length)
      : t('backup.server.checkEmpty')
  })
}

function choose(task: BackupTask, id: string): void {
  if (id === 'edit') editing.value = task
  else if (id === 'delete') deleting.value = task
  else if (id === 'restore') emit('restore', task.id)
  else if (id === 'log') emit('log', task.id)
  else if (id === 'check') void check(task)
}

async function remove(): Promise<void> {
  const task = deleting.value
  if (task) await act(task, () => actions.remove(task.id))
}
</script>

<!-- The tasks as cards: add one, restore, and for each back up now or more. -->
<template>
  <div class="grid gap-4">
    <div class="flex flex-wrap justify-end gap-2">
      <Button
        variant="outline"
        :disabled="!tasks.length"
        @click="emit('restore', tasks[0]?.id ?? 0)"
        ><ArchiveRestore /> {{ t('backup.restore.title') }}</Button
      >
      <Button @click="creating = true"><Plus /> {{ t('backup.add') }}</Button>
    </div>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <Alert v-if="notice">{{ notice }}</Alert>
    <p v-if="!tasks.length" class="rounded-xl border p-6 text-sm text-muted-foreground">
      {{ t('backup.empty') }}
    </p>
    <TaskCard
      v-for="task in tasks"
      :key="task.id"
      :task="task"
      :share-names="shareNames"
      :drive-labels="driveLabels"
      :account-names="accountNames"
      :busy="busy === task.id"
      @run="run(task)"
      @stop="stop(task)"
      @action="(id) => choose(task, id)"
    />
    <TaskDialog v-model:open="creating" :plugin="plugin" :taken="names" />
    <TaskDialog
      v-if="editing"
      v-model:open="editOpen"
      :plugin="plugin"
      :task="editing"
      :taken="names.filter((name) => name !== editing?.name)"
    />
    <ConfirmDialog
      v-model:open="deleteOpen"
      :title="t('backup.deleteTitle', { name: deleting?.name ?? '' })"
      :description="t('backup.deleteText', { folder: deleting?.folder ?? '' })"
      :confirm-label="t('backup.actions.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="remove"
    />
  </div>
</template>
