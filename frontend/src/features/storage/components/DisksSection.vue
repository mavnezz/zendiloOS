<script setup lang="ts">
import { CalendarClock, ChevronRight, HardDrive, MemoryStick } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Disk } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'

import { useDisks, useSelfTestPlans } from '../api'
import { formatHour, usageLabel } from '../format'
import DiskSmartBadge from './DiskSmartBadge.vue'
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
  <div class="grid gap-3">
    <div class="flex items-center gap-2 rounded-xl border bg-card p-3 text-sm">
      <CalendarClock class="size-5 shrink-0 text-primary" />
      <p class="min-w-0 flex-1">
        <span class="font-medium">{{ t('storage.selfTestPlans.title') }}:</span>
        {{ schedule.length ? schedule.join(' · ') : t('storage.selfTestPlans.none') }}
      </p>
      <Button v-if="canManage" variant="outline" size="sm" @click="planning = true">
        {{ t('storage.selfTestPlans.button') }}
      </Button>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !disks?.length" class="text-sm text-muted-foreground">
      {{ t('storage.noDisks') }}
    </p>
    <ul class="grid gap-2">
      <li v-for="disk in disks" :key="disk.name">
        <button
          type="button"
          class="flex w-full items-center gap-3 rounded-xl border bg-card p-3 text-left transition-colors hover:bg-accent/60"
          @click="$emit('select', disk.name)"
        >
          <component
            :is="disk.media === 'rotational' ? HardDrive : MemoryStick"
            class="size-5 shrink-0 text-primary"
          />
          <div class="min-w-0 flex-1">
            <p class="truncate font-medium">
              {{ disk.name }}
              <span class="font-normal text-muted-foreground"
                >· {{ disk.model ?? t('storage.unknownModel') }}</span
              >
            </p>
            <p class="truncate text-xs text-muted-foreground">
              {{ formatBytes(disk.size_bytes) }} · {{ t(`storage.transport.${disk.transport}`) }} ·
              {{ usage(disk) }}
            </p>
          </div>
          <DiskSmartBadge :disk="disk.name" />
          <ChevronRight class="size-4 shrink-0 text-muted-foreground" />
        </button>
      </li>
    </ul>
    <SelfTestPlansDialog v-if="canManage" v-model:open="planning" />
  </div>
</template>
