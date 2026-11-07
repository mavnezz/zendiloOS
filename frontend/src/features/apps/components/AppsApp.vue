<script setup lang="ts">
import { Container, Download, LayoutGrid, LibraryBig, Search } from 'lucide-vue-next'
import { computed, defineComponent, h, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { useWindowSection } from '@/shared/lib/window-section'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'
import { Input } from '@/shared/ui/input'

import { useAppProjects, useCatalog, useDocker, useInstalled } from '../api'
import AppDetail from './AppDetail.vue'
import DockerSection from './DockerSection.vue'
import InstalledSection from './InstalledSection.vue'
import SourcesSection from './SourcesSection.vue'
import StoreSection from './StoreSection.vue'

const { t } = useI18n()
const { data: catalog } = useCatalog()
const { data: installed } = useInstalled()
const { data: projects } = useAppProjects()
const { data: docker } = useDocker()
const dockerReady = computed(() => Boolean(docker.value?.installed && docker.value.volume))

/** Where the label of a category starts, under those with an icon. */
const Indent = defineComponent({ render: () => h('span', { 'aria-hidden': 'true' }) })

const categories = computed(() =>
  [...new Set((catalog.value ?? []).flatMap((app) => app.categories))].sort((a, b) =>
    t(`appStore.categories.${a}`).localeCompare(t(`appStore.categories.${b}`)),
  ),
)
const groups = computed<NavGroup[]>(() => [
  {
    id: 'runtime',
    items: [
      {
        id: 'docker',
        label: 'Docker',
        icon: Container,
        alert: docker.value ? !dockerReady.value : false,
      },
    ],
  },
  {
    id: 'store',
    items: [
      { id: 'all', label: t('appStore.allApps'), icon: LayoutGrid },
      ...categories.value.map((category) => ({
        id: `category:${category}`,
        label: t(`appStore.categories.${category}`),
        icon: Indent,
      })),
      {
        id: 'installed',
        label: t('appStore.sections.installed'),
        icon: Download,
        count:
          installed.value && projects.value
            ? installed.value.length + projects.value.length
            : undefined,
        alert: installed.value?.some((app) => app.state === 'failed'),
      },
    ],
  },
])
// Without Docker, the store opens on it: nothing else can be installed.
const section = useWindowSection(() => [
  ...(docker.value && !dockerReady.value ? ['docker'] : []),
  'all',
  ...categories.value.map((category) => `category:${category}`),
  'installed',
  'sources',
  'docker',
])
const category = computed(() =>
  section.value.startsWith('category:') ? section.value.slice('category:'.length) : null,
)

const search = ref('')
/** One app shown in full, perhaps with its installation begun. */
const opened = ref<{ source: number; id: string; install: boolean } | null>(null)
watch(section, () => (opened.value = null))
// Searching looks through the whole store.
watch(search, (words) => {
  if (words.trim() && (section.value === 'installed' || section.value === 'sources'))
    section.value = 'all'
  opened.value = null
})

function open(app: { source_id: number; id: string }, install = false): void {
  opened.value = { source: app.source_id, id: app.id, install }
}
</script>

<!-- Apps for the NAS, from the community catalog and others. -->
<template>
  <AppFrame v-model:section="section" :groups="groups">
    <template #sidebar-top>
      <div class="relative mb-2 hidden @2xl/app:block">
        <Search class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="search"
          class="h-8 rounded-full border-transparent bg-foreground/[0.06] pl-8"
          :placeholder="t('appStore.searchShort')"
          :aria-label="t('appStore.search')"
        />
      </div>
    </template>
    <template #sidebar-bottom>
      <button
        type="button"
        class="flex items-center gap-2 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground hover:bg-foreground/5 hover:text-foreground"
        :class="section === 'sources' && 'bg-primary/10 font-medium text-primary'"
        @click="section = 'sources'"
      >
        <LibraryBig class="size-4" />
        {{ t('appStore.sections.sources') }}
      </button>
    </template>

    <AppDetail
      v-if="opened"
      :id="opened.id"
      :key="`${opened.source}/${opened.id}`"
      :source="opened.source"
      :install="opened.install"
      @back="opened = null"
      @installed="section = 'installed'"
    />
    <DockerSection v-else-if="section === 'docker'" @store="section = 'all'" />
    <InstalledSection v-else-if="section === 'installed'" @open="open" @store="section = 'all'" />
    <SourcesSection v-else-if="section === 'sources'" />
    <StoreSection
      v-else
      :category="category"
      :search="search"
      @open="(app) => open(app)"
      @install="(app) => open(app, true)"
      @installed="section = 'installed'"
      @docker="section = 'docker'"
    />
  </AppFrame>
</template>
