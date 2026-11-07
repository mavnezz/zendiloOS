<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatPercent } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Meter } from '@/shared/ui/meter'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useVolumes } from '../api'

const { t } = useI18n()
const { data: volumes, error, isPending } = useVolumes()

const share = (used: number, size: number) => (size > 0 ? used / size : 0)
</script>

<template>
  <div class="grid gap-3">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !volumes?.length" class="text-sm text-muted-foreground">
      {{ t('storage.noVolumes') }}
    </p>
    <Table v-else>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('storage.columns.volume') }}</TableHead>
          <TableHead>{{ t('storage.columns.mount') }}</TableHead>
          <TableHead class="hidden @2xl:table-cell">{{
            t('storage.columns.filesystem')
          }}</TableHead>
          <TableHead class="w-48">{{ t('storage.columns.used') }}</TableHead>
          <TableHead class="text-right">{{ t('storage.columns.size') }}</TableHead>
          <TableHead class="text-right">{{ t('storage.columns.free') }}</TableHead>
          <TableHead class="hidden @3xl:table-cell">{{ t('storage.columns.disks') }}</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="volume in volumes" :key="volume.id">
          <TableCell class="font-mono text-xs font-medium">
            {{ volume.label ?? volume.devices.join(', ') }}
          </TableCell>
          <TableCell class="font-mono text-xs">
            {{
              volume.mount_points.length ? volume.mount_points.join(', ') : t('storage.notMounted')
            }}
          </TableCell>
          <TableCell class="hidden text-muted-foreground @2xl:table-cell">
            {{ volume.filesystem }}{{ volume.array ? ` · ${volume.array}` : '' }}
          </TableCell>
          <TableCell>
            <span
              v-if="volume.size_bytes != null && volume.used_bytes != null"
              class="flex items-center gap-2"
            >
              <Meter
                :value="share(volume.used_bytes, volume.size_bytes)"
                :label="t('storage.usedSpace')"
              />
              <span class="w-10 text-right text-xs tabular-nums">
                {{ formatPercent(share(volume.used_bytes, volume.size_bytes) * 100) }}
              </span>
            </span>
          </TableCell>
          <TableCell class="text-right tabular-nums">
            {{ volume.size_bytes != null ? formatBytes(volume.size_bytes) : '–' }}
          </TableCell>
          <TableCell class="text-right tabular-nums">
            {{ volume.available_bytes != null ? formatBytes(volume.available_bytes) : '–' }}
          </TableCell>
          <TableCell class="hidden font-mono text-xs text-muted-foreground @3xl:table-cell">
            {{ volume.disks.join(', ') }}
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
</template>
