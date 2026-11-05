<script setup lang="ts">
import { LoaderCircle, RefreshCw } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { LogPriority, LogSource } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'

import { useJournal } from '../api'

const SOURCES: LogSource[] = ['system', 'zendiloos', 'storage', 'sharing', 'kernel']
const PRIORITIES: LogPriority[] = ['error', 'warning', 'notice', 'info', 'debug']

const { t } = useI18n()
const source = ref<LogSource>('system')
const priority = ref<LogPriority>('info')
const journal = useJournal(source, priority)

const entries = computed(() => journal.data.value?.pages.flatMap((page) => page.entries) ?? [])

function tone(level: LogPriority): string {
  switch (level) {
    case 'emergency':
    case 'alert':
    case 'critical':
    case 'error':
      return 'text-destructive'
    case 'warning':
      return 'text-amber-700 dark:text-amber-400'
    case 'debug':
      return 'text-muted-foreground'
    default:
      return ''
  }
}
</script>

<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-end gap-3">
      <label class="grid gap-1 text-xs text-muted-foreground">
        {{ t('logs.source') }}
        <select
          v-model="source"
          class="h-8 rounded-md border bg-background px-2 text-sm text-foreground shadow-xs"
        >
          <option v-for="option in SOURCES" :key="option" :value="option">
            {{ t(`logs.sources.${option}`) }}
          </option>
        </select>
      </label>
      <label class="grid gap-1 text-xs text-muted-foreground">
        {{ t('logs.priority') }}
        <select
          v-model="priority"
          class="h-8 rounded-md border bg-background px-2 text-sm text-foreground shadow-xs"
        >
          <option v-for="option in PRIORITIES" :key="option" :value="option">
            {{ t(`logs.priorities.${option}`) }}
          </option>
        </select>
      </label>
      <Button variant="outline" size="sm" class="ml-auto" @click="journal.refetch()">
        <RefreshCw />
        {{ t('storage.refresh') }}
      </Button>
    </div>

    <Alert v-if="journal.error.value" variant="destructive">
      {{ problemMessage(journal.error.value) }}
    </Alert>
    <p
      v-else-if="journal.isPending.value"
      class="flex items-center gap-2 text-sm text-muted-foreground"
    >
      <LoaderCircle class="size-4 animate-spin" />
      {{ t('logs.loading') }}
    </p>
    <p v-else-if="!entries.length" class="text-sm text-muted-foreground">
      {{ t('logs.empty') }}
    </p>

    <ol v-if="entries.length" class="grid rounded-lg border font-mono text-xs">
      <li
        v-for="(entry, index) in entries"
        :key="index"
        class="grid gap-x-3 border-b px-3 py-1.5 last:border-b-0 @3xl:grid-cols-[10rem_9rem_1fr]"
      >
        <time class="text-muted-foreground tabular-nums">
          {{ formatDateTime(entry.at_micros / 1000) }}
        </time>
        <span class="truncate text-muted-foreground" :title="entry.unit ?? undefined">
          {{ entry.identifier ?? entry.unit ?? '–'
          }}<template v-if="entry.pid">[{{ entry.pid }}]</template>
        </span>
        <span :class="cn('break-words whitespace-pre-wrap', tone(entry.priority))">
          {{ entry.message }}
        </span>
      </li>
    </ol>

    <Button
      v-if="journal.hasNextPage.value"
      variant="outline"
      size="sm"
      class="justify-self-center"
      :disabled="journal.isFetchingNextPage.value"
      @click="journal.fetchNextPage()"
    >
      {{ t('logs.older') }}
    </Button>
  </div>
</template>
