<script setup lang="ts">
import { Layers } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import type { RaidArray } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDuration, formatPercent, formatRate } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge, type BadgeVariants } from '@/shared/ui/badge'
import { Meter } from '@/shared/ui/meter'

import { useArrays } from '../api'
import { formatRaidLevel } from '../format'

const { t } = useI18n()
const { data: arrays, error, isPending } = useArrays()

const HEALTH: Record<RaidArray['health'], BadgeVariants['variant']> = {
  healthy: 'success',
  degraded: 'warning',
  rebuilding: 'default',
  inactive: 'destructive',
}

const MEMBER: Record<RaidArray['members'][number]['state'], BadgeVariants['variant']> = {
  active: 'secondary',
  spare: 'outline',
  faulty: 'destructive',
  replacement: 'default',
  journal: 'outline',
}
</script>

<template>
  <div class="grid gap-3">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !arrays?.length" class="text-sm text-muted-foreground">
      {{ t('storage.noArrays') }}
    </p>
    <section
      v-for="array in arrays"
      :key="array.name"
      class="grid gap-3 rounded-xl border bg-card p-4"
    >
      <header class="flex flex-wrap items-center gap-3">
        <Layers class="size-5 shrink-0 text-primary" />
        <div class="min-w-0 flex-1">
          <h3 class="font-medium">
            <span class="font-mono text-sm font-semibold">{{ array.name }}</span> ·
            {{ formatRaidLevel(array.level) }}
          </h3>
          <p class="text-xs text-muted-foreground">
            <template v-if="array.size_bytes != null">{{ formatBytes(array.size_bytes) }}</template>
            <template v-if="array.devices_total != null">
              ·
              {{
                t('storage.devicesWorking', {
                  working: array.devices_working,
                  total: array.devices_total,
                })
              }}
            </template>
          </p>
        </div>
        <Badge v-if="array.read_only" variant="outline">{{ t('storage.readOnly') }}</Badge>
        <Badge :variant="HEALTH[array.health]">{{
          t(`storage.arrayHealth.${array.health}`)
        }}</Badge>
      </header>

      <div v-if="array.sync" class="grid gap-1.5">
        <p class="text-sm">
          {{ t(`storage.sync.${array.sync.operation}`) }}
          <template v-if="array.sync.percent != null">
            · {{ formatPercent(array.sync.percent) }}
          </template>
          <template v-else> · {{ t('storage.syncWaiting') }}</template>
        </p>
        <Meter
          v-if="array.sync.percent != null"
          :value="array.sync.percent / 100"
          :label="t(`storage.sync.${array.sync.operation}`)"
          kind="progress"
        />
        <p
          v-if="array.sync.remaining_seconds != null || array.sync.speed_bytes_per_second != null"
          class="text-xs text-muted-foreground"
        >
          <template v-if="array.sync.remaining_seconds != null">
            {{ t('storage.remaining', { time: formatDuration(array.sync.remaining_seconds) }) }}
          </template>
          <template v-if="array.sync.speed_bytes_per_second != null">
            · {{ formatRate(array.sync.speed_bytes_per_second) }}
          </template>
        </p>
      </div>

      <ul class="flex flex-wrap gap-2" :aria-label="t('storage.members')">
        <li
          v-for="member in array.members"
          :key="member.device"
          class="flex items-center gap-2 rounded-lg border px-2.5 py-1 text-sm"
        >
          <span class="font-mono text-dense font-medium">{{ member.device }}</span>
          <Badge :variant="MEMBER[member.state]">{{ t(`storage.member.${member.state}`) }}</Badge>
        </li>
      </ul>
    </section>
  </div>
</template>
