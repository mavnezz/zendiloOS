<script setup lang="ts">
import { RefreshCw } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { formatDateTime } from '@/shared/lib/format'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useActivity } from '../api'

const i18n = useI18n()
const { t } = i18n
const { data: activity, refetch, isFetching } = useActivity()

function action(name: string): string {
  const key = `docker.activity.actions.${name.replace('containers.', '')}`
  return i18n.te(key) ? t(key) : name
}
</script>

<!-- What was done with Docker here, and by whom: the audit log's entries. -->
<template>
  <div class="grid gap-3">
    <div class="flex justify-end">
      <Button variant="secondary" :disabled="isFetching" @click="refetch()">
        <RefreshCw />
        {{ t('docker.activity.refresh') }}
      </Button>
    </div>
    <div class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('docker.activity.outcome') }}</TableHead>
            <TableHead>{{ t('docker.activity.who') }}</TableHead>
            <TableHead>{{ t('docker.activity.when') }}</TableHead>
            <TableHead>{{ t('docker.activity.what') }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="entry in activity?.entries" :key="entry.id">
            <TableCell>
              <Badge :variant="entry.outcome === 'success' ? 'success' : 'destructive'">{{
                t(`docker.activity.outcomes.${entry.outcome}`)
              }}</Badge>
            </TableCell>
            <TableCell>{{ entry.actor ?? 'system' }}</TableCell>
            <TableCell class="text-muted-foreground">{{
              formatDateTime(entry.at * 1000)
            }}</TableCell>
            <TableCell>
              {{ action(entry.action) }}
              <span v-if="entry.target" class="font-medium">{{ entry.target }}</span>
              <Badge v-if="entry.reason === 'risks_accepted'" variant="warning" class="ml-2">{{
                t('docker.activity.risksAccepted')
              }}</Badge>
            </TableCell>
          </TableRow>
          <TableRow v-if="activity && !activity.entries.length" class="hover:bg-transparent">
            <TableCell colspan="4" class="py-10 text-center text-muted-foreground">{{
              t('docker.activity.none')
            }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
  </div>
</template>
