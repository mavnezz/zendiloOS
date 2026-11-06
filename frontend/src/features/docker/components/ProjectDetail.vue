<script setup lang="ts">
import {
  ArrowLeft,
  Boxes,
  Pencil,
  Play,
  RefreshCw,
  RotateCw,
  Square,
  Trash2,
} from 'lucide-vue-next'
import { computed, ref, toRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { type Action, ActionMenu } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { CodeEditor } from '@/shared/ui/code-editor'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { PageTabs } from '@/shared/ui/page'

import { useLogs, useProject, useProjectAction, useProjectCompose } from '../api'
import { projectStatus, statusVariant } from '../lib/status'
import ContainerRow from './ContainerRow.vue'
import LogView from './LogView.vue'
import UsageChart from './UsageChart.vue'

const props = defineProps<{ name: string }>()
const emit = defineEmits<{ back: []; edit: [compose: string] }>()

const { t } = useI18n()
const name = toRef(props, 'name')
const tab = ref('containers')
const { data: project, error } = useProject(name)
const app = computed(() => project.value?.kind === 'app')
const { data: compose, error: composeError } = useProjectCompose(
  name,
  () => tab.value === 'compose' && !app.value,
)
const { data: logs, isPending: logsPending } = useLogs('projects', name, () => tab.value === 'logs')
const act = useProjectAction()
const deleting = ref(false)

const names = computed(() => project.value?.containers.map((c) => c.name) ?? [])
const tabs = computed(() => [
  { id: 'containers', label: t('docker.sections.containers') },
  { id: 'usage', label: t('docker.projects.monitoring') },
  { id: 'logs', label: t('docker.projects.log') },
  { id: 'compose', label: t('docker.projects.compose') },
])
const actions = computed<Action[]>(() => [
  {
    id: 'restart',
    label: t('docker.actions.restart'),
    icon: RotateCw,
    disabled: !names.value.length,
  },
  { id: 'redeploy', label: t('docker.actions.redeploy'), icon: RefreshCw, disabled: app.value },
  {
    id: 'delete',
    label: t('docker.actions.delete'),
    icon: Trash2,
    destructive: true,
    disabled: app.value,
  },
])

function run(action: string): void {
  if (action === 'delete') {
    deleting.value = true
    return
  }
  act.mutate({ name: props.name, action: action as 'start' | 'stop' | 'restart' | 'redeploy' })
}

function remove(): void {
  act.mutate({ name: props.name, action: 'delete' }, { onSuccess: () => emit('back') })
}
</script>

<!-- One Compose project: its containers, their use, their output, its file. -->
<template>
  <div class="flex min-h-full flex-col">
    <div class="mb-4 flex flex-wrap items-center gap-3">
      <Button variant="ghost" size="icon" :aria-label="t('docker.back')" @click="emit('back')">
        <ArrowLeft />
      </Button>
      <span class="grid size-12 place-items-center rounded-xl bg-primary/10 text-primary">
        <Boxes class="size-6" />
      </span>
      <div class="grid min-w-0 flex-1 gap-0.5">
        <span class="flex flex-wrap items-center gap-2">
          <span class="text-lg font-semibold">{{ name }}</span>
          <Badge v-if="project" :variant="statusVariant(projectStatus(project))">{{
            t(`docker.status.${projectStatus(project)}`)
          }}</Badge>
          <Badge v-if="project" variant="outline">{{ t(`docker.kinds.${project.kind}`) }}</Badge>
        </span>
        <span class="truncate text-xs text-muted-foreground">
          <template v-if="project?.created_at">
            {{
              t('docker.projects.created', {
                at: formatDateTime(project.created_at * 1000),
                by: project.created_by,
              })
            }}
            ·
          </template>
          <span class="font-mono">{{ project?.files.join(', ') }}</span>
        </span>
      </div>
      <Button
        variant="ghost"
        size="sm"
        :disabled="!project || !names.length || project.running === names.length"
        @click="run('start')"
      >
        <Play />
        {{ t('docker.actions.start') }}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        :disabled="!project || project.running === 0"
        @click="run('stop')"
      >
        <Square />
        {{ t('docker.actions.stop') }}
      </Button>
      <ActionMenu :actions="actions" :label="t('docker.actions.more')" @select="run" />
    </div>
    <Alert v-if="error || act.error.value" variant="destructive" class="mb-3">{{
      problemMessage(error ?? act.error.value)
    }}</Alert>
    <Alert v-if="app" class="mb-3">{{ t('docker.projects.appHint') }}</Alert>
    <PageTabs v-model="tab" :label="name" :tabs="tabs" />

    <ul v-if="tab === 'containers'" class="grid gap-3">
      <li
        v-for="container in project?.containers"
        :key="container.id"
        class="rounded-xl border bg-card p-4"
      >
        <ContainerRow :container="container" />
      </li>
      <li
        v-if="project && !project.containers.length"
        class="py-12 text-center text-sm text-muted-foreground"
      >
        {{ t('docker.projects.noContainers') }}
      </li>
    </ul>
    <UsageChart v-else-if="tab === 'usage'" :containers="names" :seconds="900" />
    <LogView v-else-if="tab === 'logs'" :text="logs?.text" :loading="logsPending" />
    <div v-else-if="tab === 'compose'" class="grid gap-3">
      <Alert v-if="app">{{ t('docker.projects.appCompose') }}</Alert>
      <Alert v-else-if="composeError" variant="destructive">{{
        problemMessage(composeError)
      }}</Alert>
      <template v-for="file in compose?.files" :key="file.path">
        <p class="font-mono text-xs text-muted-foreground">{{ file.path }}</p>
        <CodeEditor :model-value="file.content" :label="file.path" readonly />
      </template>
      <div v-if="compose?.editable" class="flex justify-end">
        <Button @click="emit('edit', compose.files[0]?.content ?? '')">
          <Pencil />
          {{ t('docker.projects.change') }}
        </Button>
      </div>
      <p v-else-if="compose" class="text-sm text-muted-foreground">
        {{ t('docker.projects.foreignCompose') }}
      </p>
    </div>

    <ConfirmDialog
      v-model:open="deleting"
      :title="t('docker.projects.deleteTitle', { name })"
      :description="t('docker.projects.deleteText')"
      :confirm-label="t('docker.actions.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="remove"
    />
  </div>
</template>
