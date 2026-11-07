<script setup lang="ts">
import { toRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useContainerProcesses } from '../api'

const props = defineProps<{ id: string; running: boolean }>()
const { t } = useI18n()
const {
  data: processes,
  error,
  isPending,
} = useContainerProcesses(toRef(props, 'id'), () => props.running)
</script>

<!-- What runs in a container, as the machine sees it. -->
<template>
  <p v-if="!running" class="py-12 text-center text-sm text-muted-foreground">
    {{ t('docker.details.notRunning') }}
  </p>
  <Alert v-else-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
  <div v-else-if="isPending" class="h-32 animate-pulse rounded-xl bg-muted" />
  <Table v-else>
    <TableHeader>
      <TableRow>
        <TableHead>{{ t('docker.processes.pid') }}</TableHead>
        <TableHead>{{ t('docker.processes.user') }}</TableHead>
        <TableHead class="text-right">CPU</TableHead>
        <TableHead class="text-right">{{ t('docker.processes.memory') }}</TableHead>
        <TableHead>{{ t('docker.processes.elapsed') }}</TableHead>
        <TableHead>{{ t('docker.processes.command') }}</TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableRow v-for="process in processes" :key="process.pid">
        <TableCell class="font-mono text-xs tabular-nums">{{ process.pid }}</TableCell>
        <TableCell class="font-mono text-xs">{{ process.user }}</TableCell>
        <TableCell class="text-right tabular-nums"
          >{{ process.cpu_percent.toFixed(1) }} %</TableCell
        >
        <TableCell class="text-right tabular-nums">{{
          formatBytes(process.resident_bytes)
        }}</TableCell>
        <TableCell class="font-mono text-xs tabular-nums">{{ process.elapsed }}</TableCell>
        <TableCell class="max-w-0 truncate font-mono text-xs" :title="process.command">{{
          process.command
        }}</TableCell>
      </TableRow>
    </TableBody>
  </Table>
</template>
