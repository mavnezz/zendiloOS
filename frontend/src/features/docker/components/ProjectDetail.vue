<script setup lang="ts">
import {
  ArrowLeft,
  Boxes,
  FileCode2,
  Pencil,
  Play,
  RefreshCw,
  RotateCw,
  Square,
  Trash2,
} from 'lucide-vue-next'
import { computed, inject, ref, toRef } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Job } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatDateTime } from '@/shared/lib/format'
import { WINDOW_SECTION } from '@/shared/lib/window-section'
import { type Action, ActionMenu } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { CodeEditor } from '@/shared/ui/code-editor'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { PageTabs } from '@/shared/ui/page'

import { useProject, useProjectAction, useProjectCompose, useRedeploy, useUsage } from '../api'
import { projectStatus, statusVariant } from '../lib/status'
import ContainerRow from './ContainerRow.vue'
import DeployLogDialog from './DeployLogDialog.vue'
import ProjectLog from './ProjectLog.vue'
import ProjectUsage from './ProjectUsage.vue'
import RedeployDialog from './RedeployDialog.vue'

const props = defineProps<{ name: string }>()
const emit = defineEmits<{ back: []; edit: [compose: string]; container: [name: string] }>()

/** The page of one container in this window: `container:<name>`. */
const CONTAINER = 'container:'

const { t } = useI18n()
const name = toRef(props, 'name')
const tab = ref('containers')
const { data: project, error } = useProject(name)
const app = computed(() => project.value?.kind === 'app')
const { data: compose, error: composeError } = useProjectCompose(
  name,
  () => tab.value === 'compose' && !app.value,
)
const act = useProjectAction()
const redeploy = useRedeploy()
const jobs = useJobsStore()
const deleting = ref(false)
const asking = ref(false)
const deploying = ref<number | null>(null)
const showing = ref(false)

const names = computed(() => project.value?.containers.map((c) => c.name) ?? [])
// What the network does now: from the last of the 20-second samples.
const { data: recent } = useUsage(ref(60), names)
const rates = computed(() => {
  const last = recent.value?.at(-1)
  return new Map(
    (last?.points ?? []).map((point) => [
      point.container,
      { sent: point.sent_per_second, received: point.received_per_second },
    ]),
  )
})
const where = computed(() => project.value?.compose_file ?? project.value?.files.join(', ') ?? '')
const tabs = computed(() => [
  { id: 'containers', label: t('docker.sections.containers') },
  { id: 'usage', label: t('docker.projects.monitoring') },
  { id: 'logs', label: t('docker.projects.log') },
  { id: 'compose', label: t('docker.projects.compose') },
])
const actions = computed<Action[]>(() => [
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
  if (action === 'delete') deleting.value = true
  else if (action === 'redeploy') asking.value = true
  else act.mutate({ name: props.name, action: action as 'start' | 'stop' | 'restart' })
}

function remove(): void {
  act.mutate({ name: props.name, action: 'delete' }, { onSuccess: () => emit('back') })
}

/** Redeploys, then shows what Compose writes as it does. */
function deploy(pull: boolean): void {
  redeploy.mutate(
    { name: props.name, pull },
    {
      onSuccess: (job: Job) => {
        jobs.update(job)
        deploying.value = job.id
        asking.value = false
        showing.value = true
      },
    },
  )
}

const frame = inject(WINDOW_SECTION, null)
/** Opens the container's own page. */
function open(container: string): void {
  emit('container', container)
  frame?.navigate(`${CONTAINER}${container}`)
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
        <span class="flex flex-wrap gap-x-3 text-xs text-muted-foreground">
          <span v-if="project?.created_at">{{
            t('docker.projects.created', {
              at: formatDateTime(project.created_at * 1000),
              by: project.created_by,
            })
          }}</span>
          <span v-if="where" class="flex min-w-0 items-center gap-1">
            <FileCode2 class="size-3.5 shrink-0" />
            <span class="sr-only">{{ t('docker.projects.composeFile') }}</span>
            <span class="truncate font-mono" data-testid="compose-file">{{ where }}</span>
          </span>
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
      <Button variant="ghost" size="sm" :disabled="!names.length" @click="run('restart')">
        <RotateCw />
        {{ t('docker.actions.restart') }}
      </Button>
      <ActionMenu :actions="actions" :label="t('docker.actions.more')" @select="run" />
    </div>
    <Alert
      v-if="error || act.error.value || redeploy.error.value"
      variant="destructive"
      class="mb-3"
      >{{ problemMessage(error ?? act.error.value ?? redeploy.error.value) }}</Alert
    >
    <Alert v-if="app" class="mb-3">{{ t('docker.projects.appHint') }}</Alert>
    <PageTabs v-model="tab" :label="name" :tabs="tabs" />

    <ul v-if="tab === 'containers'" class="grid gap-3">
      <li v-for="container in project?.containers" :key="container.id">
        <div
          role="button"
          tabindex="0"
          class="cursor-pointer rounded-xl border bg-card p-4 transition-colors outline-none hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring"
          :aria-label="t('docker.projects.openContainer', { name: container.name })"
          @click="open(container.name)"
          @keydown.enter.prevent="open(container.name)"
          @keydown.space.prevent="open(container.name)"
        >
          <ContainerRow :container="container" :rates="rates.get(container.name)" />
        </div>
      </li>
      <li
        v-if="project && !project.containers.length"
        class="py-12 text-center text-sm text-muted-foreground"
      >
        {{ t('docker.projects.noContainers') }}
      </li>
    </ul>
    <ProjectUsage v-else-if="tab === 'usage'" :containers="names" />
    <ProjectLog v-else-if="tab === 'logs'" :name="name" :containers="names" />
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
    <RedeployDialog
      v-model:open="asking"
      :name="name"
      :busy="redeploy.isPending.value"
      @confirm="deploy"
    />
    <DeployLogDialog v-model:open="showing" :job-id="deploying" />
  </div>
</template>
