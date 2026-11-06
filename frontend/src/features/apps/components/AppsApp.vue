<script setup lang="ts">
import { LayoutGrid, LibraryBig, Store } from 'lucide-vue-next'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CatalogApp } from '@/shared/api/types'
import { cn } from '@/shared/lib/utils'

import AppDetail from './AppDetail.vue'
import InstalledSection from './InstalledSection.vue'
import SourcesSection from './SourcesSection.vue'
import StoreSection from './StoreSection.vue'

type Section = 'store' | 'installed' | 'sources'

const { t } = useI18n()
const sections = [
  { id: 'store', icon: Store },
  { id: 'installed', icon: LayoutGrid },
  { id: 'sources', icon: LibraryBig },
] as const
const section = ref<Section>('store')
const opened = ref<{ source: number; id: string } | null>(null)

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
  <div class="flex h-full flex-col @xl:flex-row">
    <nav class="flex shrink-0 gap-1 border-b p-2 @xl:w-48 @xl:flex-col @xl:border-r @xl:border-b-0">
      <button
        v-for="item in sections"
        :key="item.id"
        type="button"
        :class="
          cn(
            'flex items-center gap-2 rounded-md px-3 py-2 text-sm',
            section === item.id ? 'bg-accent font-medium' : 'hover:bg-accent/60',
          )
        "
        :aria-current="section === item.id ? 'page' : undefined"
        @click="choose(item.id)"
      >
        <component :is="item.icon" class="size-4" />
        {{ t(`appStore.sections.${item.id}`) }}
      </button>
    </nav>
    <div class="min-w-0 flex-1 overflow-auto p-4 @xl:p-6">
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
    </div>
  </div>
</template>
