<script setup lang="ts">
import { ScrollText, ShieldCheck } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { useWindowSection } from '@/shared/lib/window-section'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'

import AuditSection from './AuditSection.vue'
import JournalSection from './JournalSection.vue'

const { t } = useI18n()
const groups = computed<NavGroup[]>(() => [
  {
    id: 'logs',
    items: [
      { id: 'journal', label: t('logs.sections.journal'), icon: ScrollText },
      { id: 'audit', label: t('logs.sections.audit'), icon: ShieldCheck },
    ],
  },
])
const section = useWindowSection(() => ['journal', 'audit'])
</script>

<!-- What the system logged, and who changed what. -->
<template>
  <AppFrame v-model:section="section" :groups="groups">
    <JournalSection v-if="section === 'journal'" />
    <AuditSection v-else />
  </AppFrame>
</template>
