<script setup lang="ts">
import {
  ArchiveRestore,
  HardDriveDownload,
  LayoutDashboard,
  ListChecks,
  ScrollText,
  Settings2,
} from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { useOpenApp, useWindowSection } from '@/shared/lib/window-section'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'

import { useBackupPlugin, useBackupTasks } from '../api'
import { statusOf } from '../lib/backup'
import LogSection from './LogSection.vue'
import OverviewSection from './OverviewSection.vue'
import RestoreSection from './RestoreSection.vue'
import SettingsSection from './SettingsSection.vue'
import TasksSection from './TasksSection.vue'

const SECTIONS = ['overview', 'tasks', 'restore', 'log', 'settings'] as const

const { t } = useI18n()
const openApp = useOpenApp()
const { data: plugin, error } = useBackupPlugin()
const ready = computed(() => plugin.value?.installed === true)
const { data: tasks } = useBackupTasks(() => ready.value)
const section = useWindowSection(() => [...SECTIONS])
/** A task another page chose to restore from or to show the log of. */
const focus = ref<number | null>(null)

const groups = computed<NavGroup[]>(() => [
  {
    id: 'backup',
    items: [
      { id: 'overview', label: t('backup.sections.overview'), icon: LayoutDashboard },
      {
        id: 'tasks',
        label: t('backup.sections.tasks'),
        icon: ListChecks,
        count: tasks.value?.length,
        alert: tasks.value?.some((task) => ['failed', 'unreachable'].includes(statusOf(task))),
      },
      { id: 'restore', label: t('backup.sections.restore'), icon: ArchiveRestore },
      { id: 'log', label: t('backup.sections.log'), icon: ScrollText },
      { id: 'settings', label: t('backup.sections.settings'), icon: Settings2 },
    ],
  },
])

function show(next: 'restore' | 'log' | 'tasks', task: number | null = null): void {
  focus.value = task
  section.value = next
}
</script>

<!-- Backup: tasks to another place on the NAS or a server, their log, restores. -->
<template>
  <AppFrame v-model:section="section" :groups="ready ? groups : undefined">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <div
      v-if="plugin && !ready"
      class="grid justify-items-start gap-3 rounded-xl border p-6 text-sm"
      data-testid="backup-not-ready"
    >
      <HardDriveDownload class="size-8 text-muted-foreground" />
      <p class="text-muted-foreground">{{ t('backup.notReady') }}</p>
      <Button v-if="openApp" size="sm" variant="outline" @click="openApp('apps', 'backup')">
        {{ t('backup.toAppCenter') }}
      </Button>
    </div>
    <template v-else-if="plugin">
      <TasksSection
        v-if="section === 'tasks'"
        :tasks="tasks ?? []"
        :plugin="plugin"
        @restore="(id) => show('restore', id)"
        @log="(id) => show('log', id)"
      />
      <RestoreSection v-else-if="section === 'restore'" :tasks="tasks ?? []" :task="focus" />
      <LogSection v-else-if="section === 'log'" :tasks="tasks ?? []" :task="focus" />
      <SettingsSection v-else-if="section === 'settings'" />
      <OverviewSection v-else :tasks="tasks ?? []" @tasks="show('tasks')" />
    </template>
  </AppFrame>
</template>
