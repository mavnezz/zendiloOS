<script setup lang="ts">
import { LayoutGrid, LibraryBig, Store } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CatalogApp } from '@/shared/api/types'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'

import { useInstalled } from '../api'

import AppDetail from './AppDetail.vue'
import InstalledSection from './InstalledSection.vue'
import SourcesSection from './SourcesSection.vue'
import StoreSection from './StoreSection.vue'

type Section = 'store' | 'installed' | 'sources'

const { t } = useI18n()
const { data: installed } = useInstalled()
const groups = computed<NavGroup[]>(() => [
  {
    id: 'apps',
    items: [
      { id: 'store', label: t('appStore.sections.store'), icon: Store },
      {
        id: 'installed',
        label: t('appStore.sections.installed'),
        icon: LayoutGrid,
        count: installed.value?.length,
        alert: installed.value?.some((app) => app.update || app.state === 'failed'),
      },
      { id: 'sources', label: t('appStore.sections.sources'), icon: LibraryBig },
    ],
  },
])
const section = ref<string>('store')
const opened = ref<{ source: number; id: string } | null>(null)
watch(section, () => (opened.value = null))

function choose(next: Section): void {
  section.value = next
  opened.value = null
}

function open(app: CatalogApp): void {
  opened.value = { source: app.source_id, id: app.id }
}
</script>

<!-- Apps for the NAS, from the community catalog and others. -->
<template>
  <AppFrame v-model:section="section" :groups="groups">
    <template v-if="section === 'store'">
      <AppDetail
        v-if="opened"
        :id="opened.id"
        :source="opened.source"
        @back="opened = null"
        @installed="choose('installed')"
      />
      <StoreSection v-else @open="open" />
    </template>
    <InstalledSection v-else-if="section === 'installed'" @store="choose('store')" />
    <SourcesSection v-else />
  </AppFrame>
</template>
