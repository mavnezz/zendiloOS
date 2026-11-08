<script setup lang="ts">
import { CircleAlert, CircleCheck, Info, TriangleAlert } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Pool, StorageConcern, StorageSeverity } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatPercent } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Meter } from '@/shared/ui/meter'
import { PanelSection } from '@/shared/ui/section'
import { StatTile } from '@/shared/ui/stat'

import { useEnclosure, usePools, useStorageOverview } from '../api'
import { formatPoolLayout } from '../format'
import EnclosureSchematic from './EnclosureSchematic.vue'

defineEmits<{ select: [name: string] }>()

const i18n = useI18n()
const { t } = i18n
const { data: overview, error } = useStorageOverview()
const { data: pools } = usePools()
const { data: enclosure } = useEnclosure()

const icons: Record<StorageSeverity, unknown> = {
  notice: Info,
  warning: TriangleAlert,
  critical: CircleAlert,
}
const tones: Record<StorageSeverity, string> = {
  notice: 'text-info',
  warning: 'text-warning',
  critical: 'text-destructive',
}

function used(pool: Pool): number {
  const total = (pool.used_bytes ?? 0) + (pool.available_bytes ?? 0)
  return total > 0 ? (pool.used_bytes ?? 0) / total : 0
}

const verdict = computed(() => overview.value?.verdict ?? null)
const total = computed(() =>
  overview.value ? overview.value.used_bytes + overview.value.available_bytes : 0,
)

/** The concern in the interface language; unknown codes as they are. */
function wording(concern: StorageConcern): string {
  const key = `storage.concerns.${concern.code}`
  if (!i18n.te(key)) return concern.code
  const params: Record<string, string> = { ...concern.params }
  if (params.state) params.state = t(`notices.states.${params.state}`)
  if (params.model === '') params.model = '?'
  return t(key, params)
}
</script>

<!-- What the storage is and what is wrong with it, the worst first. -->
<template>
  <div class="grid gap-6">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <template v-else-if="overview">
      <section
        :class="
          cn(
            'flex items-center gap-3 rounded-lg border-l-[3px] px-4 py-3',
            verdict === 'critical' && 'border-destructive bg-destructive/[0.06]',
            verdict === 'warning' && 'border-warning bg-warning/[0.08]',
            verdict === 'notice' && 'border-info bg-info/[0.06]',
            !verdict && 'border-success bg-success/[0.07]',
          )
        "
        role="status"
      >
        <component
          :is="verdict ? icons[verdict] : CircleCheck"
          :class="cn('size-5 shrink-0', verdict ? tones[verdict] : 'text-success')"
        />
        <div class="grid">
          <h3 class="text-sm font-semibold">{{ t(`storage.verdict.${verdict ?? 'good'}`) }}</h3>
          <p class="text-xs text-muted-foreground">
            {{
              verdict
                ? t('storage.concernCount', overview.concerns.length)
                : t('storage.verdict.goodDetail')
            }}
          </p>
        </div>
      </section>

      <div class="grid grid-cols-2 gap-4 @2xl:grid-cols-4">
        <StatTile
          :label="t('storage.summary.pools')"
          :value="String(overview.pools)"
          tone="primary"
        />
        <StatTile
          :label="t('storage.summary.disks')"
          :value="String(overview.disks)"
          tone="chart-5"
        />
        <StatTile
          :label="t('storage.summary.used')"
          :value="formatBytes(overview.used_bytes)"
          :hint="total > 0 ? formatPercent((overview.used_bytes / total) * 100) : undefined"
          tone="chart-3"
        />
        <StatTile
          :label="t('storage.summary.available')"
          :value="formatBytes(overview.available_bytes)"
          :hint="total > 0 ? t('storage.ofTotal', { total: formatBytes(total) }) : undefined"
          tone="chart-2"
        />
      </div>

      <PanelSection v-if="enclosure?.source" :title="t('storage.enclosure.title')">
        <EnclosureSchematic
          :enclosure="enclosure"
          compact
          class="justify-self-start"
          @select="$emit('select', $event)"
        />
      </PanelSection>

      <PanelSection v-if="overview.concerns.length" :title="t('storage.concernsTitle')">
        <ul class="divide-y rounded-lg border">
          <li
            v-for="(concern, index) in overview.concerns"
            :key="index"
            class="flex items-center gap-3 px-3 py-2"
          >
            <component
              :is="icons[concern.severity]"
              :class="['size-4 shrink-0', tones[concern.severity]]"
            />
            <p class="text-sm">{{ wording(concern) }}</p>
          </li>
        </ul>
      </PanelSection>

      <PanelSection v-if="pools?.length" :title="t('storage.pools')">
        <ul class="grid gap-3 @3xl:grid-cols-2">
          <li
            v-for="pool in pools"
            :key="pool.guid"
            class="grid gap-2 rounded-lg border px-3 py-2.5"
          >
            <div class="flex items-center gap-2">
              <span class="font-mono text-sm font-semibold">{{ pool.name }}</span>
              <span class="truncate text-xs text-muted-foreground">
                {{ formatPoolLayout(pool) }} · {{ pool.mount_point ?? '–' }}
              </span>
              <span class="flex-1" />
              <Badge
                :variant="
                  pool.state === 'online'
                    ? 'success'
                    : pool.state === 'degraded'
                      ? 'warning'
                      : 'destructive'
                "
              >
                {{ t(`storage.vdevState.${pool.state}`) }}
              </Badge>
            </div>
            <div class="flex items-center gap-2">
              <Meter :value="used(pool)" :label="t('storage.usedSpace')" />
              <span class="w-10 text-right text-xs tabular-nums">{{
                formatPercent(used(pool) * 100)
              }}</span>
            </div>
            <p class="text-xs text-muted-foreground tabular-nums">
              {{ formatBytes(pool.used_bytes ?? 0) }} /
              {{ formatBytes((pool.used_bytes ?? 0) + (pool.available_bytes ?? 0)) }} ·
              {{ t('storage.diskCount', pool.devices.length) }}
            </p>
          </li>
        </ul>
      </PanelSection>
    </template>
  </div>
</template>
