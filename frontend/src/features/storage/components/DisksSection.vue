<script setup lang="ts">
import { CalendarClock, ChevronRight, HardDrive, MemoryStick } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Disk } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useDisks, useSelfTestPlans } from '../api'
import { formatHour, usageLabel } from '../format'
import DiskSmartCells from './DiskSmartCells.vue'
import SelfTestPlansDialog from './SelfTestPlansDialog.vue'

/** Whether the signed-in account may plan self-tests (`storage.manage`). */
defineProps<{ canManage: boolean }>()
defineEmits<{ select: [name: string] }>()

const { t } = useI18n()
const { data: disks, error, isPending } = useDisks()
const { data: plans } = useSelfTestPlans()
const planning = ref(false)

const schedule = computed(() =>
  (plans.value?.plans ?? []).map((plan) =>
    t('storage.selfTestPlans.summary', {
      kind: t(`storage.selfTest.${plan.kind}`),
      frequency: t(`storage.selfTestPlans.frequency.${plan.frequency}`),
      hour: formatHour(plan.hour),
    }),
  ),
)

function usage(disk: Disk): string {
  return usageLabel(disk)
}
</script>

<template>
  <div class="grid gap-4">
    <div class="flex items-center gap-2 rounded-lg bg-muted/50 px-3 py-2 text-xs">
      <CalendarClock class="size-4 shrink-0 text-muted-foreground" />
      <p class="min-w-0 flex-1 truncate">
        <span class="font-medium">{{ t('storage.selfTestPlans.title') }}:</span>
        {{ schedule.length ? schedule.join(' · ') : t('storage.selfTestPlans.none') }}
      </p>
      <Button v-if="canManage" variant="soft" size="sm" shape="pill" @click="planning = true">
        {{ t('storage.selfTestPlans.button') }}
      </Button>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !disks?.length" class="text-sm text-muted-foreground">
      {{ t('storage.noDisks') }}
    </p>
    <Table v-else>
      <TableHeader>
        <TableRow>
          <TableHead>{{ t('storage.columns.device') }}</TableHead>
          <TableHead>{{ t('storage.columns.model') }}</TableHead>
          <TableHead class="hidden @4xl:table-cell">{{ t('storage.columns.serial') }}</TableHead>
          <TableHead class="text-right">{{ t('storage.columns.size') }}</TableHead>
          <TableHead class="hidden @2xl:table-cell">{{ t('storage.columns.bus') }}</TableHead>
          <TableHead>{{ t('storage.columns.usage') }}</TableHead>
          <TableHead class="text-right">{{ t('storage.columns.temperature') }}</TableHead>
          <TableHead class="text-right">{{ t('storage.columns.age') }}</TableHead>
          <TableHead>{{ t('storage.columns.health') }}</TableHead>
          <TableHead class="w-8" />
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="disk in disks"
          :key="disk.name"
          class="cursor-pointer"
          tabindex="0"
          @click="$emit('select', disk.name)"
          @keydown.enter="$emit('select', disk.name)"
        >
          <TableCell>
            <span class="flex items-center gap-2">
              <component
                :is="disk.media === 'rotational' ? HardDrive : MemoryStick"
                class="size-4 shrink-0 text-muted-foreground"
              />
              <span class="font-mono text-xs font-medium">{{ disk.name }}</span>
            </span>
          </TableCell>
          <TableCell class="max-w-56 truncate">{{
            disk.model ?? t('storage.unknownModel')
          }}</TableCell>
          <TableCell class="hidden font-mono text-xs text-muted-foreground @4xl:table-cell">
            {{ disk.serial ?? '–' }}
          </TableCell>
          <TableCell class="text-right whitespace-nowrap tabular-nums">{{
            formatBytes(disk.size_bytes)
          }}</TableCell>
          <TableCell class="hidden text-muted-foreground @2xl:table-cell">
            {{ t(`storage.transport.${disk.transport}`) }}
          </TableCell>
          <TableCell class="text-muted-foreground">{{ usage(disk) }}</TableCell>
          <DiskSmartCells :disk="disk.name" />
          <TableCell class="text-muted-foreground"><ChevronRight class="size-4" /></TableCell>
        </TableRow>
      </TableBody>
    </Table>
    <SelfTestPlansDialog v-if="canManage" v-model:open="planning" />
  </div>
</template>
