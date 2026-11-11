<script setup lang="ts">
import { CircleAlert, CircleCheck, CircleDashed, LoaderCircle } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BackupTask } from '@/shared/api/types'
import { formatDateTime } from '@/shared/lib/format'
import { ActionMenu, type Action } from '@/shared/ui/action-menu'
import { Button } from '@/shared/ui/button'

import { statusOf } from '../lib/backup'
import { placeText, scheduleText } from '../lib/describe'

const props = defineProps<{
  task: BackupTask
  shareNames: Map<number, string>
  driveLabels: Map<string, string>
  busy?: boolean
}>()
const emit = defineEmits<{ run: []; stop: []; action: [id: string] }>()

const { t } = useI18n()
const status = computed(() => statusOf(props.task))
const when = (seconds: number) => formatDateTime(seconds * 1000)
const schedule = computed(() => {
  const text = scheduleText(t, props.task.schedule, props.task.on_plug)
  return props.task.next_at != null
    ? t('backup.card.next', { schedule: text, when: when(props.task.next_at) })
    : text
})
const actions = computed<Action[]>(() => [
  { id: 'edit', label: t('backup.actions.edit'), disabled: props.task.running },
  ...(props.task.server ? [{ id: 'check', label: t('backup.actions.check') }] : []),
  { id: 'restore', label: t('backup.actions.restore'), disabled: props.task.running },
  { id: 'log', label: t('backup.actions.log') },
  {
    id: 'delete',
    label: t('backup.actions.delete'),
    destructive: true,
    disabled: props.task.running,
  },
])
</script>

<!-- One task: its state, last run, places, mode and schedule; back up now, or more. -->
<template>
  <article class="grid gap-4 rounded-xl border bg-card p-4" :data-task="task.id">
    <header class="flex flex-wrap items-start gap-3">
      <div class="grid min-w-0 flex-1 gap-1">
        <h3 class="truncate font-semibold">{{ task.name }}</h3>
        <p class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
          <span
            class="flex items-center gap-1.5 font-medium"
            :class="{
              'text-emerald-600 dark:text-emerald-400': status === 'succeeded',
              'text-sky-600 dark:text-sky-400': status === 'running',
              'text-amber-600 dark:text-amber-400':
                status === 'partial' || status === 'unreachable',
              'text-destructive': status === 'failed',
              'text-muted-foreground': status === 'never',
            }"
          >
            <CircleCheck v-if="status === 'succeeded'" class="size-4" />
            <LoaderCircle v-else-if="status === 'running'" class="size-4 animate-spin" />
            <CircleDashed v-else-if="status === 'never'" class="size-4" />
            <CircleAlert v-else class="size-4" />
            {{ t(`backup.status.${status}`) }}
          </span>
          <span class="text-muted-foreground">{{
            task.last_run
              ? t('backup.card.lastRun', { when: when(task.last_run.started_at) })
              : t('backup.card.never')
          }}</span>
        </p>
      </div>
      <Button
        v-if="task.running"
        size="sm"
        variant="outline"
        :disabled="busy"
        @click="emit('stop')"
        >{{ t('backup.stop') }}</Button
      >
      <Button
        v-else
        size="sm"
        variant="outline"
        :disabled="busy || !task.reachable"
        @click="emit('run')"
        >{{ t('backup.backUpNow') }}</Button
      >
      <ActionMenu
        :actions="actions"
        :label="t('backup.more', { name: task.name })"
        @select="(id) => emit('action', id)"
      />
    </header>
    <p
      v-if="task.last_run?.error && status !== 'running'"
      class="rounded-md bg-muted p-2 text-xs text-muted-foreground"
    >
      {{ task.last_run.error }}
    </p>
    <dl class="grid gap-3 border-t pt-3 text-sm @2xl:grid-cols-2">
      <div class="grid min-w-0 gap-0.5">
        <dt class="text-xs text-muted-foreground">{{ t('backup.card.source') }}</dt>
        <dd class="truncate">
          {{ placeText(t, task.source, task.server, shareNames, driveLabels) }}
        </dd>
      </div>
      <div class="grid min-w-0 gap-0.5">
        <dt class="text-xs text-muted-foreground">{{ t('backup.card.target') }}</dt>
        <dd class="truncate">
          {{ placeText(t, task.target, task.server, shareNames, driveLabels) }}
        </dd>
      </div>
      <div class="grid gap-0.5">
        <dt class="text-xs text-muted-foreground">{{ t('backup.card.mode') }}</dt>
        <dd>{{ t(`backup.modes.${task.mode.kind}`) }} · {{ t(`backup.kinds.${task.kind}`) }}</dd>
      </div>
      <div class="grid gap-0.5">
        <dt class="text-xs text-muted-foreground">{{ t('backup.card.schedule') }}</dt>
        <dd>{{ schedule }}</dd>
      </div>
    </dl>
  </article>
</template>
