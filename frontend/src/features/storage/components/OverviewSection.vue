<script setup lang="ts">
import { CircleAlert, CircleCheck, Info, TriangleAlert } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { StorageConcern, StorageSeverity } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Meter } from '@/shared/ui/meter'

import { useStorageOverview } from '../api'

const i18n = useI18n()
const { t } = i18n
const { data: overview, error } = useStorageOverview()

const icons: Record<StorageSeverity, unknown> = {
  notice: Info,
  warning: TriangleAlert,
  critical: CircleAlert,
}
const tones: Record<StorageSeverity, string> = {
  notice: 'text-primary',
  warning: 'text-amber-600 dark:text-amber-400',
  critical: 'text-destructive',
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
  <div class="grid gap-4">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <template v-else-if="overview">
      <section
        :class="
          cn(
            'flex items-start gap-3 rounded-xl border p-4',
            verdict === 'critical' && 'border-destructive/50 bg-destructive/5',
            verdict === 'warning' && 'border-amber-500/50 bg-amber-500/5',
          )
        "
        role="status"
      >
        <component
          :is="verdict ? icons[verdict] : CircleCheck"
          :class="
            cn(
              'mt-0.5 size-6 shrink-0',
              verdict ? tones[verdict] : 'text-emerald-600 dark:text-emerald-400',
            )
          "
        />
        <div>
          <h3 class="font-semibold">{{ t(`storage.verdict.${verdict ?? 'good'}`) }}</h3>
          <p v-if="!verdict" class="text-sm text-muted-foreground">
            {{ t('storage.verdict.goodDetail') }}
          </p>
        </div>
      </section>

      <ul v-if="overview.concerns.length" class="divide-y rounded-xl border bg-card">
        <li
          v-for="(concern, index) in overview.concerns"
          :key="index"
          class="flex items-start gap-3 px-4 py-3"
        >
          <component
            :is="icons[concern.severity]"
            :class="['mt-0.5 size-4 shrink-0', tones[concern.severity]]"
          />
          <p class="text-sm">{{ wording(concern) }}</p>
        </li>
      </ul>

      <dl class="grid grid-cols-2 gap-3 @2xl:grid-cols-4">
        <div class="rounded-xl border bg-card p-4">
          <dt class="text-xs text-muted-foreground">{{ t('storage.summary.pools') }}</dt>
          <dd class="text-2xl font-semibold tabular-nums">{{ overview.pools }}</dd>
        </div>
        <div class="rounded-xl border bg-card p-4">
          <dt class="text-xs text-muted-foreground">{{ t('storage.summary.disks') }}</dt>
          <dd class="text-2xl font-semibold tabular-nums">{{ overview.disks }}</dd>
        </div>
        <div class="rounded-xl border bg-card p-4">
          <dt class="text-xs text-muted-foreground">{{ t('storage.summary.used') }}</dt>
          <dd class="text-2xl font-semibold tabular-nums">
            {{ formatBytes(overview.used_bytes) }}
          </dd>
        </div>
        <div class="rounded-xl border bg-card p-4">
          <dt class="text-xs text-muted-foreground">{{ t('storage.summary.available') }}</dt>
          <dd class="text-2xl font-semibold tabular-nums">
            {{ formatBytes(overview.available_bytes) }}
          </dd>
        </div>
      </dl>
      <Meter
        v-if="total > 0"
        :value="overview.used_bytes / total"
        :label="t('storage.usedSpace')"
      />
    </template>
  </div>
</template>
