<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SmartReport } from '@/shared/api/types'
import { Badge, type BadgeVariants } from '@/shared/ui/badge'

const props = defineProps<{ report?: SmartReport; loading?: boolean }>()

const { t } = useI18n()

const shown = computed<{ variant: BadgeVariants['variant']; label: string }>(() => {
  const report = props.report
  if (!report) {
    return { variant: 'outline', label: props.loading ? t('storage.smartLoading') : '–' }
  }
  switch (report.status) {
    case 'standby':
      return { variant: 'secondary', label: t('storage.smart.standby') }
    case 'unsupported':
      return { variant: 'outline', label: t('storage.smart.unsupported') }
    default:
      break
  }
  switch (report.health) {
    case 'failing':
      return { variant: 'destructive', label: t('storage.health.failing') }
    case 'warning':
      return { variant: 'warning', label: t('storage.health.warning') }
    default:
      return { variant: 'success', label: t('storage.health.good') }
  }
})
</script>

<template>
  <Badge :variant="shown.variant">{{ shown.label }}</Badge>
</template>
