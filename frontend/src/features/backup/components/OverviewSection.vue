<script setup lang="ts">
import { CircleAlert, CircleCheck, HardDriveDownload, LoaderCircle } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BackupTask } from '@/shared/api/types'
import { formatDateTime } from '@/shared/lib/format'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'

import { overview, statusOf } from '../lib/backup'
import { placeText } from '../lib/describe'
import { useNames } from '../names'

const props = defineProps<{ tasks: BackupTask[] }>()
const emit = defineEmits<{ tasks: [] }>()

const { t } = useI18n()
const { shareNames, driveLabels, accountNames } = useNames()
const glance = computed(() => overview(props.tasks))
const when = (seconds: number) => formatDateTime(seconds * 1000)

/** Each task's target and whether it is there now. */
const targets = computed(() =>
  props.tasks.map((task) => ({
    task,
    text: placeText(
      t,
      task.target,
      task.server,
      shareNames.value,
      driveLabels.value,
      accountNames.value,
    ),
    // Looked at when it runs.
    server: task.target.kind === 'server' || task.target.kind === 'cloud',
  })),
)
</script>

<!-- How the backups stand: in a word, the last and next runs, the targets. -->
<template>
  <div class="grid gap-6">
    <header class="flex items-center gap-4 border-b pb-5" data-testid="backup-state">
      <span
        class="grid size-14 shrink-0 place-items-center rounded-full"
        :class="{
          'bg-muted text-success': glance.state === 'fine',
          'bg-sky-500/15 text-sky-600': glance.state === 'running',
          'bg-amber-500/15 text-amber-600': glance.state === 'attention',
          'bg-muted text-muted-foreground': glance.state === 'empty',
        }"
      >
        <CircleCheck v-if="glance.state === 'fine'" class="size-8" />
        <LoaderCircle v-else-if="glance.state === 'running'" class="size-8 animate-spin" />
        <CircleAlert v-else-if="glance.state === 'attention'" class="size-8" />
        <HardDriveDownload v-else class="size-8" />
      </span>
      <div class="grid gap-0.5">
        <h2 class="display text-xl">{{ t(`backup.overview.states.${glance.state}`) }}</h2>
        <p v-if="glance.state === 'empty'" class="text-sm text-muted-foreground">
          {{ t('backup.overview.emptyHint') }}
        </p>
        <p v-else-if="glance.running" class="text-sm text-muted-foreground">
          {{ t('backup.overview.runningCount', glance.running) }}
        </p>
      </div>
      <Button v-if="glance.state === 'empty'" class="ml-auto" @click="emit('tasks')">{{
        t('backup.add')
      }}</Button>
    </header>

    <dl v-if="tasks.length" class="grid gap-4 text-sm @2xl:grid-cols-2">
      <div class="grid gap-1 rounded-xl border p-4">
        <dt class="text-xs text-muted-foreground">{{ t('backup.overview.lastRun') }}</dt>
        <dd v-if="glance.last" class="grid gap-0.5">
          <span class="font-medium">{{ glance.last.task.name }}</span>
          <span class="text-muted-foreground"
            >{{ when(glance.last.run.started_at) }} ·
            {{ t(`backup.log.outcomes.${glance.last.run.outcome}`) }}</span
          >
        </dd>
        <dd v-else>{{ t('backup.overview.none') }}</dd>
      </div>
      <div class="grid gap-1 rounded-xl border p-4">
        <dt class="text-xs text-muted-foreground">{{ t('backup.overview.nextRun') }}</dt>
        <dd v-if="glance.next" class="grid gap-0.5">
          <span class="font-medium">{{ glance.next.task.name }}</span>
          <span class="text-muted-foreground">{{ when(glance.next.at) }}</span>
        </dd>
        <dd v-else>{{ t('backup.overview.none') }}</dd>
      </div>
    </dl>

    <section v-if="glance.attention.length" class="grid gap-2">
      <h3 class="text-base font-semibold">{{ t('backup.overview.attention') }}</h3>
      <ul class="grid gap-2 text-sm">
        <li
          v-for="task in glance.attention"
          :key="task.id"
          class="flex flex-wrap items-center gap-2 rounded-lg border border-amber-500/40 p-3"
        >
          <span class="font-medium">{{ task.name }}</span>
          <Badge variant="warning">{{ t(`backup.status.${statusOf(task)}`) }}</Badge>
          <span v-if="task.last_run?.error" class="w-full truncate text-muted-foreground">{{
            task.last_run.error
          }}</span>
        </li>
      </ul>
    </section>

    <section v-if="targets.length" class="grid gap-2">
      <h3 class="text-base font-semibold">{{ t('backup.overview.targets') }}</h3>
      <ul class="grid gap-2 text-sm @2xl:grid-cols-2">
        <li
          v-for="target in targets"
          :key="target.task.id"
          class="grid gap-1 rounded-lg border p-3"
        >
          <span class="flex items-center gap-2">
            <span
              class="size-2 rounded-full"
              :class="
                target.server
                  ? 'bg-muted-foreground'
                  : target.task.reachable
                    ? 'bg-success'
                    : 'bg-destructive'
              "
            />
            <span class="font-medium">{{ target.task.name }}</span>
          </span>
          <span class="truncate text-muted-foreground">{{ target.text }}</span>
          <span class="text-xs text-muted-foreground">{{
            target.server
              ? t('backup.overview.serverUnknown')
              : target.task.reachable
                ? t('backup.overview.reachable')
                : t('backup.overview.unreachable')
          }}</span>
        </li>
      </ul>
    </section>
  </div>
</template>
