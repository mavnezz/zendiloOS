<script setup lang="ts">
import { ChevronRight, HardDrive, MemoryStick } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import type { Disk } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'

import { useDisks } from '../api'
import { usageLabel } from '../format'
import DiskSmartBadge from './DiskSmartBadge.vue'

defineEmits<{ select: [name: string] }>()

const { t } = useI18n()
const { data: disks, error, isPending } = useDisks()

function usage(disk: Disk): string {
  return usageLabel(disk)
}
</script>

<template>
  <div class="grid gap-3">
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
  </div>
</template>
