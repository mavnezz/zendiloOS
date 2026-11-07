<script setup lang="ts">
import { RefreshCw } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AuditRecord } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge, type BadgeVariants } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'

import { useAuditLog } from '../api'

const i18n = useI18n()
const { t } = i18n
const audit = useAuditLog()

const records = computed(() => audit.data.value?.pages.flatMap((page) => page.entries) ?? [])

const OUTCOME: Record<string, BadgeVariants['variant']> = {
  success: 'success',
  failure: 'warning',
  denied: 'destructive',
}

/** A readable name for known actions, the dotted name otherwise. */
function action(record: AuditRecord): string {
  const key = `logs.actions.${record.action.replace('.', '_')}`
  return i18n.te(key) ? t(key) : record.action
}

function outcome(record: AuditRecord): string {
  const key = `logs.outcomes.${record.outcome}`
  return i18n.te(key) ? t(key) : record.outcome
}
</script>

<template>
  <div class="grid gap-3">
    <div class="flex items-center justify-end gap-2">
      <Button variant="outline" size="sm" @click="audit.refetch()">
        <RefreshCw />
        {{ t('storage.refresh') }}
      </Button>
    </div>
    <Alert v-if="audit.error.value" variant="destructive">
      {{ problemMessage(audit.error.value) }}
    </Alert>
    <p v-else-if="!audit.isPending.value && !records.length" class="text-sm text-muted-foreground">
      {{ t('logs.empty') }}
    </p>
    <div v-if="records.length" class="overflow-x-auto rounded-lg border">
      <table class="w-full text-sm">
        <thead class="bg-muted/50 text-left text-xs text-muted-foreground">
          <tr>
            <th class="px-3 py-2 font-medium">{{ t('logs.time') }}</th>
            <th class="px-3 py-2 font-medium">{{ t('logs.actor') }}</th>
            <th class="px-3 py-2 font-medium">{{ t('logs.action') }}</th>
            <th class="px-3 py-2 font-medium">{{ t('logs.outcome') }}</th>
            <th class="px-3 py-2 font-medium">{{ t('logs.address') }}</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="record in records" :key="record.id" class="border-t align-top">
            <td class="px-3 py-1.5 whitespace-nowrap text-muted-foreground tabular-nums">
              {{ formatDateTime(record.at * 1000) }}
            </td>
            <td class="px-3 py-1.5">{{ record.actor ?? '–' }}</td>
            <td class="px-3 py-1.5">
              {{ action(record) }}
              <span v-if="record.target" class="text-muted-foreground">· {{ record.target }}</span>
              <span v-if="record.reason" class="block text-xs text-muted-foreground">
                {{ record.reason }}
              </span>
            </td>
            <td class="px-3 py-1.5">
              <Badge :variant="OUTCOME[record.outcome] ?? 'outline'">
                {{ outcome(record) }}
              </Badge>
            </td>
            <td class="px-3 py-1.5 font-mono text-xs text-muted-foreground">
              {{ record.client_ip ?? '–' }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <Button
      v-if="audit.hasNextPage.value"
      variant="outline"
      size="sm"
      class="justify-self-center"
      :disabled="audit.isFetchingNextPage.value"
      @click="audit.fetchNextPage()"
    >
      {{ t('logs.older') }}
    </Button>
  </div>
</template>
