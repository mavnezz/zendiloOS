<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BackupRun, BackupTask, BackupTransfers } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'

import { transfers, useBackupRuns } from '../api'
import { changeKind } from '../lib/describe'

/** The runs of every task or one (`task`), each with its transfer list. */
const props = defineProps<{ tasks: BackupTask[]; task: number | null }>()

const { t } = useI18n()
const shown = ref<number | null>(props.task)
watch(
  () => props.task,
  (id) => (shown.value = id),
)
const { data: runs, error } = useBackupRuns(() => shown.value)
const names = computed(() => new Map(props.tasks.map((task) => [task.id, task.name])))
const when = (seconds: number) => formatDateTime(seconds * 1000)

const open = ref(false)
const details = ref<BackupTransfers | null>(null)
const failure = ref<string | null>(null)
const search = ref('')
const found = computed(() =>
  (details.value?.transfers ?? []).filter((transfer) =>
    transfer.path.toLowerCase().includes(search.value.trim().toLowerCase()),
  ),
)

const VARIANTS: Record<string, 'success' | 'warning' | 'destructive' | 'secondary' | 'info'> = {
  succeeded: 'success',
  partial: 'warning',
  failed: 'destructive',
  stopped: 'secondary',
  interrupted: 'warning',
  running: 'info',
}

async function show(run: BackupRun): Promise<void> {
  failure.value = null
  search.value = ''
  try {
    details.value = await transfers(run.id)
    open.value = true
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}
</script>

<template>
  <div class="grid gap-4">
    <div class="flex items-center gap-2">
      <NativeSelect
        id="backup-log-task"
        :model-value="shown ?? ''"
        class="w-64"
        @update:model-value="(id) => (shown = id === '' ? null : Number(id))"
      >
        <option value="">{{ t('backup.log.allTasks') }}</option>
        <option v-for="candidate in tasks" :key="candidate.id" :value="candidate.id">
          {{ candidate.name }}
        </option>
      </NativeSelect>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <p v-if="runs && !runs.length" class="text-sm text-muted-foreground">
      {{ t('backup.log.empty') }}
    </p>
    <div v-else-if="runs" class="overflow-x-auto rounded-xl border">
      <table class="w-full text-sm">
        <thead class="bg-muted/50 text-left text-xs text-muted-foreground">
          <tr>
            <th class="px-3 py-2 font-medium">{{ t('backup.log.task') }}</th>
            <th class="px-3 py-2 font-medium">{{ t('backup.log.started') }}</th>
            <th class="px-3 py-2 font-medium">{{ t('backup.log.trigger') }}</th>
            <th class="px-3 py-2 font-medium">{{ t('backup.log.outcome') }}</th>
            <th class="px-3 py-2 font-medium">{{ t('backup.log.transferred') }}</th>
            <th class="px-3 py-2" />
          </tr>
        </thead>
        <tbody>
          <tr v-for="run in runs" :key="run.id" class="border-t align-top" :data-run="run.id">
            <td class="px-3 py-2 font-medium">{{ names.get(run.task_id) ?? `#${run.task_id}` }}</td>
            <td class="px-3 py-2 whitespace-nowrap">
              {{ when(run.started_at) }}
              <span v-if="run.finished_at" class="block text-xs text-muted-foreground"
                >{{ t('backup.log.ended') }}: {{ when(run.finished_at) }}</span
              >
            </td>
            <td class="px-3 py-2">{{ run.by ?? t(`backup.log.triggers.${run.trigger}`) }}</td>
            <td class="px-3 py-2">
              <Badge :variant="VARIANTS[run.outcome]">{{
                t(`backup.log.outcomes.${run.outcome}`)
              }}</Badge>
              <span
                v-if="run.error"
                class="mt-1 block max-w-md text-xs break-words text-muted-foreground"
                >{{ run.error }}</span
              >
            </td>
            <td class="px-3 py-2 whitespace-nowrap">
              {{ t('backup.log.files', run.files) }} · {{ formatBytes(run.bytes) }}
            </td>
            <td class="px-3 py-2 text-right">
              <Button size="sm" variant="ghost" @click="show(run)">{{
                t('backup.log.details')
              }}</Button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <ModalDialog
      v-model:open="open"
      :title="
        details
          ? t('backup.log.transfersTitle', {
              name: names.get(details.run.task_id) ?? '',
              when: when(details.run.started_at),
            })
          : ''
      "
      :close-label="t('desktop.close')"
    >
      <div v-if="details" class="grid gap-3 text-sm">
        <p v-if="details.run.version" class="text-muted-foreground">
          {{ t('backup.log.version', { version: details.run.version }) }}
        </p>
        <div v-if="details.run.error" class="grid gap-1">
          <span class="font-medium">{{ t('backup.log.error') }}</span>
          <pre class="max-h-40 overflow-auto rounded-md bg-muted p-2 text-xs whitespace-pre-wrap">{{
            details.run.error
          }}</pre>
        </div>
        <p v-if="!details.run.transfers" class="text-muted-foreground">
          {{ t('backup.log.noTransfers') }}
        </p>
        <template v-else>
          <p class="text-xs text-muted-foreground">
            {{
              t('backup.log.transfersKept', {
                shown: details.transfers.length,
                count: details.run.transfers,
                kept: 5000,
              })
            }}
          </p>
          <Input
            v-model="search"
            :placeholder="t('backup.log.search')"
            :aria-label="t('backup.log.search')"
          />
          <ul class="grid max-h-80 overflow-y-auto font-mono text-xs">
            <li v-for="(transfer, index) in found" :key="index" class="flex gap-2 border-b py-1">
              <span class="w-20 shrink-0 font-sans text-muted-foreground">{{
                t(`backup.log.changes.${changeKind(transfer.change)}`)
              }}</span>
              <span class="break-all">{{ transfer.path }}</span>
            </li>
          </ul>
        </template>
      </div>
    </ModalDialog>
  </div>
</template>
