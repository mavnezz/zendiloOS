<script setup lang="ts">
import { Box, Boxes, Disc3, LayoutDashboard, Network, ScrollText, Settings2 } from 'lucide-vue-next'
import { computed, inject, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { useWindowSection, WINDOW_SECTION } from '@/shared/lib/window-section'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'

import ActivitySection from './ActivitySection.vue'
import ContainersSection from './ContainersSection.vue'
import ImagesSection from './ImagesSection.vue'
import ManagementSection from './ManagementSection.vue'
import NetworksSection from './NetworksSection.vue'
import OverviewSection from './OverviewSection.vue'
import ProjectDetail from './ProjectDetail.vue'
import ProjectEditor from './ProjectEditor.vue'
import ProjectsSection from './ProjectsSection.vue'

const SECTIONS = [
  'overview',
  'projects',
  'containers',
  'images',
  'networks',
  'activity',
  'management',
] as const
/** A page that is a project: `project:<name>`. */
const PROJECT = 'project:'

const { t } = useI18n()
const icons = {
  overview: LayoutDashboard,
  projects: Boxes,
  containers: Box,
  images: Disc3,
  networks: Network,
  activity: ScrollText,
  management: Settings2,
}
const groups = computed<NavGroup[]>(() => [
  {
    id: 'docker',
    items: SECTIONS.map((id) => ({ id, label: t(`docker.sections.${id}`), icon: icons[id] })),
  },
])
const section = useWindowSection(() => [...SECTIONS])

/** Within Projects: one opened, or one being made or changed. */
type View =
  | { kind: 'list' }
  | { kind: 'detail'; name: string }
  | { kind: 'create' }
  | { kind: 'edit'; name: string; compose: string }
const view = ref<View>({ kind: 'list' })
watch(section, () => (view.value = { kind: 'list' }))

// Another app (the App Center) opens a project here as `project:<name>`.
const frame = inject(WINDOW_SECTION, null)
watch(
  () => frame?.section.value,
  (asked) => {
    if (!asked?.startsWith(PROJECT)) return
    const name = asked.slice(PROJECT.length)
    section.value = 'projects'
    // After the change of page, which shows the list.
    void nextTick(() => (view.value = { kind: 'detail', name }))
  },
  { immediate: true },
)
</script>

<!-- Docker: what runs on the engine, whoever started it, and the projects made here. -->
<template>
  <AppFrame v-model:section="section" :groups="groups">
    <OverviewSection v-if="section === 'overview'" />
    <template v-else-if="section === 'projects'">
      <ProjectDetail
        v-if="view.kind === 'detail'"
        :key="view.name"
        :name="view.name"
        @back="view = { kind: 'list' }"
        @edit="
          (compose) => view.kind === 'detail' && (view = { kind: 'edit', name: view.name, compose })
        "
      />
      <ProjectEditor
        v-else-if="view.kind === 'create'"
        @back="view = { kind: 'list' }"
        @done="(name) => (view = { kind: 'detail', name })"
      />
      <ProjectEditor
        v-else-if="view.kind === 'edit'"
        :name="view.name"
        :compose="view.compose"
        @back="view.kind === 'edit' && (view = { kind: 'detail', name: view.name })"
        @done="(name) => (view = { kind: 'detail', name })"
      />
      <ProjectsSection
        v-else
        @open="(name) => (view = { kind: 'detail', name })"
        @create="view = { kind: 'create' }"
      />
    </template>
    <ContainersSection v-else-if="section === 'containers'" />
    <ImagesSection v-else-if="section === 'images'" />
    <NetworksSection v-else-if="section === 'networks'" />
    <ActivitySection v-else-if="section === 'activity'" />
    <ManagementSection v-else-if="section === 'management'" />
  </AppFrame>
</template>
