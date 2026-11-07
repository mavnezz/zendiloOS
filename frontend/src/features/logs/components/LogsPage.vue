<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { PageHeader } from '@/shared/ui/page'

import AuditSection from './AuditSection.vue'
import JournalSection from './JournalSection.vue'

defineProps<{ page: 'journal' | 'audit' }>()

const { t } = useI18n()
</script>

<!-- What the system logged, or who changed what: one page of the control panel. -->
<template>
  <div>
    <PageHeader :title="t(`logs.sections.${page}`)" :description="t(`logs.descriptions.${page}`)" />
    <JournalSection v-if="page === 'journal'" />
    <AuditSection v-else />
  </div>
</template>
