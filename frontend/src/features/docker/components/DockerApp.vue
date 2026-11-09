<script setup lang="ts">
import {
  Box,
  Boxes,
  Database,
  DatabaseBackup,
  Disc3,
  LayoutDashboard,
  Network,
  ScrollText,
  Settings2,
} from 'lucide-vue-next'
import { computed, inject, provide, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { useWindowSection, WINDOW_SECTION } from '@/shared/lib/window-section'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'

import { OPEN_CONTAINER } from '../lib/navigation'
import ActivitySection from './ActivitySection.vue'
import BackupsSection from './BackupsSection.vue'
import ContainerDetail from './ContainerDetail.vue'
import ContainersSection from './ContainersSection.vue'
import ImagesSection from './ImagesSection.vue'
import ManagementSection from './ManagementSection.vue'
import NetworksSection from './NetworksSection.vue'
import OverviewSection from './OverviewSection.vue'
import ProjectDetail from './ProjectDetail.vue'
import ProjectEditor from './ProjectEditor.vue'
import ProjectsSection from './ProjectsSection.vue'
import VolumesSection from './VolumesSection.vue'

const SECTIONS = [
  'overview',
  'projects',
  'containers',
  'images',
  'networks',
  'volumes',
  'backups',
  'activity',
  'management',
] as const
/** A page that is a project: `project:<name>`. */
const PROJECT = 'project:'
/** A page that is a container: `container:<name>`. */
const CONTAINER = 'container:'

const { t } = useI18n()
const icons = {
  overview: LayoutDashboard,
  projects: Boxes,
  containers: Box,
  images: Disc3,
  networks: Network,
  volumes: Database,
  backups: DatabaseBackup,
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
/** Within Containers: the one opened, by name, and the page it was opened from. */
const container = ref<{ name: string; from: string | null } | null>(null)
/**
 * A container to show once Containers is the page: the window may take a
 * render or two to get there, passing other pages on the way.
 */
let wanted: { name: string; from: string | null } | null = null
/** Likewise, a project to show once Projects is the page. */
let wantedProject: string | null = null
watch(section, (now) => {
  view.value =
    now === 'projects' && wantedProject ? { kind: 'detail', name: wantedProject } : { kind: 'list' }
  container.value = now === 'containers' ? wanted : null
  if (now === 'projects') wantedProject = null
  if (now === 'containers') wanted = null
})

/** Opens a project's page, from wherever the window is. */
function openProject(name: string): void {
  if (section.value === 'projects') {
    view.value = { kind: 'detail', name }
    return
  }
  wantedProject = name
  section.value = 'projects'
}

/**
 * Opens a container's page, remembering the project whose page it was
 * opened from (if it was), where Back leads.
 */
function openContainer(name: string): void {
  const from =
    container.value?.name === name
      ? container.value.from
      : view.value.kind === 'detail'
        ? view.value.name
        : null
  if (section.value === 'containers') {
    container.value = { name, from }
    return
  }
  wanted = { name, from }
  section.value = 'containers'
}

function leaveContainer(): void {
  const from = container.value?.from
  container.value = null
  if (from) openProject(from)
}

// A container's name opens its page, from Containers or a project.
provide(OPEN_CONTAINER, openContainer)

// Another app (the App Center) opens a project here as `project:<name>`.
const frame = inject(WINDOW_SECTION, null)
watch(
  () => frame?.section.value,
  (asked) => {
    if (!asked?.startsWith(PROJECT)) return
    openProject(asked.slice(PROJECT.length))
  },
  { immediate: true },
)
// A project's page opens a container as `container:<name>`. At once, while
// the project shown is still known: the page changes after.
watch(
  () => frame?.section.value,
  (asked) => {
    if (asked?.startsWith(CONTAINER)) openContainer(asked.slice(CONTAINER.length))
  },
  { immediate: true, flush: 'sync' },
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
    <template v-else-if="section === 'containers'">
      <ContainerDetail
        v-if="container"
        :key="container.name"
        :name="container.name"
        @back="leaveContainer"
      />
      <ContainersSection v-else />
    </template>
    <ImagesSection v-else-if="section === 'images'" />
    <NetworksSection v-else-if="section === 'networks'" />
    <VolumesSection v-else-if="section === 'volumes'" />
    <BackupsSection v-else-if="section === 'backups'" />
    <ActivitySection v-else-if="section === 'activity'" />
    <ManagementSection v-else-if="section === 'management'" />
  </AppFrame>
</template>
