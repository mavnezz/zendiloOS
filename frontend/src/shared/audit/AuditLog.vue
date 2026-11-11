<script setup lang="ts">
import { ChevronLeft, ChevronRight, Download, RefreshCw, Search } from 'lucide-vue-next'
import { computed, onScopeDispose, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AuditRecord } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge, type BadgeVariants } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { AUDIT_PAGE, allAuditRecords, useAuditPages } from './audit'
import { auditCsv } from './csv'

/**
 * The audit log, or an app's part of it (`prefix`): searched, a page at a
 * time, exported as CSV. `action` says an entry's action in words (the
 * dotted name otherwise); `file` names the export.
 */
const props = withDefaults(
  defineProps<{
    prefix?: string
    action?: (record: AuditRecord) => string | undefined
    file?: string
    size?: number
  }>(),
  { prefix: undefined, action: undefined, file: 'zendiloos-audit-log', size: AUDIT_PAGE },
)

const i18n = useI18n()
const { t } = i18n

/** What is typed, and what is searched a moment after typing stopped. */
const typed = ref('')
const search = ref('')
let pause: ReturnType<typeof setTimeout> | undefined
watch(typed, (text) => {
  clearTimeout(pause)
  pause = setTimeout(() => (search.value = text.trim()), 300)
})
onScopeDispose(() => clearTimeout(pause))

const filter = computed(() => ({ prefix: props.prefix, search: search.value }))
const audit = useAuditPages(filter, props.size)
const page = ref(0)
watch(filter, () => (page.value = 0))

const pages = computed(() => audit.data.value?.pages ?? [])
const records = computed(() => pages.value[page.value]?.entries ?? [])
const hasOlder = computed(() => page.value + 1 < pages.value.length || audit.hasNextPage.value)

async function older(): Promise<void> {
  if (page.value + 1 >= pages.value.length) await audit.fetchNextPage()
  if (page.value + 1 < pages.value.length) page.value++
}

const OUTCOME: Record<string, BadgeVariants['variant']> = {
  success: 'success',
  failure: 'warning',
  denied: 'destructive',
}

function actionText(record: AuditRecord): string {
  const own = props.action?.(record)
  if (own) return own
  const key = `logs.actions.${record.action.replace('.', '_')}`
  return i18n.te(key) ? t(key) : record.action
}

function outcome(record: AuditRecord): string {
  const key = `logs.outcomes.${record.outcome}`
  return i18n.te(key) ? t(key) : record.outcome
}

const exporting = ref(false)
const failure = ref<string | null>(null)

async function exportCsv(): Promise<void> {
  exporting.value = true
  failure.value = null
  try {
    const all = await allAuditRecords(filter.value)
    const text = auditCsv(
      all,
      {
        time: t('logs.time'),
        actor: t('logs.actor'),
        action: t('logs.action'),
        target: t('logs.target'),
        outcome: t('logs.outcome'),
        address: t('logs.address'),
      },
      { action: actionText, outcome, time: (record) => formatDateTime(record.at * 1000) },
    )
    // A byte order mark first, so spreadsheets read the umlauts as such.
    const url = URL.createObjectURL(new Blob(['﻿', text], { type: 'text/csv;charset=utf-8' }))
    const link = document.createElement('a')
    link.href = url
    link.download = `${props.file}.csv`
    link.click()
    URL.revokeObjectURL(url)
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    exporting.value = false
  }
}
</script>

<!-- Who did what, and how it ended: searched, a page at a time, exported as CSV. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <div class="relative min-w-48 flex-1">
        <Search
          class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="typed"
          type="search"
          class="pl-8"
          maxlength="100"
          :placeholder="t('logs.searchPlaceholder')"
          :aria-label="t('logs.search')"
          data-testid="audit-search"
        />
      </div>
      <Button
        variant="outline"
        size="sm"
        :disabled="audit.isFetching.value"
        @click="audit.refetch()"
      >
        <RefreshCw />
        {{ t('storage.refresh') }}
      </Button>
      <Button
        variant="outline"
        size="sm"
        :disabled="exporting || !records.length"
        data-testid="audit-export"
        @click="exportCsv"
      >
        <Download />
        {{ exporting ? t('logs.exporting') : t('logs.export') }}
      </Button>
    </div>
    <Alert v-if="audit.error.value" variant="destructive">
      {{ problemMessage(audit.error.value) }}
    </Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <p
      v-else-if="!audit.isPending.value && !audit.error.value && !records.length"
      class="py-6 text-center text-sm text-muted-foreground"
    >
      {{ t('logs.empty') }}
    </p>
    <div v-if="records.length" class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('logs.time') }}</TableHead>
            <TableHead>{{ t('logs.actor') }}</TableHead>
            <TableHead>{{ t('logs.action') }}</TableHead>
            <TableHead>{{ t('logs.outcome') }}</TableHead>
            <TableHead class="hidden @md:table-cell">{{ t('logs.address') }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow
            v-for="record in records"
            :key="record.id"
            class="align-top"
            data-testid="audit-row"
          >
            <TableCell class="whitespace-nowrap text-muted-foreground tabular-nums">
              {{ formatDateTime(record.at * 1000) }}
            </TableCell>
            <TableCell>{{ record.actor ?? '–' }}</TableCell>
            <TableCell>
              {{ actionText(record) }}
              <span v-if="record.target" class="font-medium">· {{ record.target }}</span>
              <span v-if="record.reason" class="block text-xs text-muted-foreground">
                {{ record.reason }}
              </span>
            </TableCell>
            <TableCell>
              <Badge :variant="OUTCOME[record.outcome] ?? 'outline'">{{ outcome(record) }}</Badge>
            </TableCell>
            <TableCell class="hidden font-mono text-xs text-muted-foreground @md:table-cell">
              {{ record.client_ip ?? '–' }}
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <div v-if="page > 0 || hasOlder" class="flex items-center justify-center gap-2">
      <Button variant="outline" size="sm" :disabled="page === 0" @click="page--">
        <ChevronLeft />
        {{ t('logs.newer') }}
      </Button>
      <span class="text-sm text-muted-foreground tabular-nums">{{
        t('logs.page', { n: page + 1 })
      }}</span>
      <Button
        variant="outline"
        size="sm"
        :disabled="!hasOlder || audit.isFetchingNextPage.value"
        data-testid="audit-older"
        @click="older"
      >
        {{ t('logs.olderPage') }}
        <ChevronRight />
      </Button>
    </div>
  </div>
</template>
