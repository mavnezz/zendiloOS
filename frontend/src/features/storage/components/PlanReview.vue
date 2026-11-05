<script setup lang="ts">
import { TriangleAlert } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import type { PlanWarning } from '@/shared/api/types'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'

/** The disks a plan erases, and its warnings. */
defineProps<{
  disks: {
    name: string
    model?: string | null
    serial?: string | null
    size_bytes: number
    erases: string[]
  }[]
  warnings: PlanWarning[]
}>()

const { t } = useI18n()
</script>

<template>
  <div class="grid gap-3">
    <Alert v-for="warning in warnings" :key="warning.code">
      <TriangleAlert class="text-amber-600 dark:text-amber-400" />
      {{
        t(`storage.plan.warning.${warning.code}`, {
          unused: formatBytes(warning.unused_bytes ?? 0),
        })
      }}
    </Alert>
    <div class="grid gap-1.5">
      <p class="text-sm font-medium text-destructive">{{ t('storage.plan.willErase') }}</p>
      <ul class="grid gap-1.5">
        <li
          v-for="disk in disks"
          :key="disk.name"
          class="grid gap-0.5 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-sm"
        >
          <span class="font-medium">
            {{ disk.name }} · {{ disk.model ?? t('storage.unknownModel') }} ·
            {{ formatBytes(disk.size_bytes) }}
          </span>
          <span class="font-mono text-xs text-muted-foreground">{{ disk.serial ?? '–' }}</span>
          <span v-if="disk.erases.length" class="text-xs text-destructive">
            {{ t('storage.plan.contains', { what: disk.erases.join(', ') }) }}
          </span>
        </li>
      </ul>
    </div>
  </div>
</template>
