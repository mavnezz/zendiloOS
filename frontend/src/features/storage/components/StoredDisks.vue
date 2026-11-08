<script setup lang="ts">
import { Trash2 } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { PanelSection } from '@/shared/ui/section'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { forgetDisk, useInventory, useRefreshStorage } from '../api'

/** `canManage`: may remove them from the list (`storage.manage`). */
defineProps<{ canManage: boolean }>()

const { t } = useI18n()
const { data: inventory } = useInventory()
const refresh = useRefreshStorage()
const failure = ref<string | null>(null)

const stored = computed(() => (inventory.value ?? []).filter((disk) => !disk.disk))
const today = new Date().toISOString().slice(0, 10)

async function forget(serial: string): Promise<void> {
  failure.value = null
  try {
    await forgetDisk(serial)
    await refresh()
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}
</script>

<!-- The disks the NAS has seen that are no longer in it, kept with what was noted about them. -->
<template>
  <PanelSection
    v-if="stored.length"
    :title="t('storage.inventory.stored')"
    :description="t('storage.inventory.storedHint')"
  >
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('storage.columns.serial') }}</TableHead>
          <TableHead>{{ t('storage.columns.model') }}</TableHead>
          <TableHead class="hidden text-right @2xl:table-cell">
            {{ t('storage.columns.size') }}
          </TableHead>
          <TableHead class="hidden @3xl:table-cell">{{
            t('storage.inventory.lastSeen')
          }}</TableHead>
          <TableHead>{{ t('storage.inventory.warrantyUntil') }}</TableHead>
          <TableHead v-if="canManage" class="w-8" />
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="disk in stored" :key="disk.serial">
          <TableCell class="font-mono text-xs">{{ disk.serial }}</TableCell>
          <TableCell class="max-w-56 truncate">{{
            disk.model ?? t('storage.unknownModel')
          }}</TableCell>
          <TableCell class="hidden text-right tabular-nums @2xl:table-cell">
            {{ formatBytes(disk.size_bytes) }}
          </TableCell>
          <TableCell class="hidden text-muted-foreground @3xl:table-cell">
            {{ formatDateTime(disk.last_seen * 1000) }}
          </TableCell>
          <TableCell>
            {{ disk.warranty_until ?? '–' }}
            <span
              v-if="disk.warranty_until && disk.warranty_until < today"
              class="text-xs text-muted-foreground"
            >
              ({{ t('storage.inventory.warrantyEnded') }})
            </span>
          </TableCell>
          <TableCell v-if="canManage">
            <Button
              variant="ghost"
              size="icon"
              :aria-label="t('storage.inventory.forget', { serial: disk.serial })"
              @click="forget(disk.serial)"
            >
              <Trash2 />
            </Button>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </PanelSection>
</template>
