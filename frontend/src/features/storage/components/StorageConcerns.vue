<script setup lang="ts">
import { CircleAlert, Info, TriangleAlert } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { StorageConcern, StorageSeverity } from '@/shared/api/types'
import { cn } from '@/shared/lib/utils'

import { useStorageOverview } from '../api'

const i18n = useI18n()
const { t } = i18n
const { data: overview } = useStorageOverview()

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

const verdict = computed(() => overview.value?.verdict ?? null)

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

<!-- What is wrong with the storage, the worst first; nothing while all is well. -->
<template>
  <section
    v-if="verdict && overview"
    :class="
      cn(
        'grid gap-1.5 rounded-lg border-l-[3px] px-4 py-3',
        verdict === 'critical' && 'border-destructive bg-destructive/[0.06]',
        verdict === 'warning' && 'border-warning bg-warning/[0.08]',
        verdict === 'notice' && 'border-info bg-info/[0.06]',
      )
    "
    role="status"
  >
    <h3 class="text-sm font-semibold">{{ t(`storage.verdict.${verdict}`) }}</h3>
    <ul class="grid gap-1">
      <li v-for="(concern, index) in overview.concerns" :key="index" class="flex items-start gap-2">
        <component
          :is="icons[concern.severity]"
          :class="cn('mt-0.5 size-4 shrink-0', tones[concern.severity])"
        />
        <p class="text-sm">{{ wording(concern) }}</p>
      </li>
    </ul>
  </section>
</template>
