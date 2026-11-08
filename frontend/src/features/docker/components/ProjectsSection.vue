<script setup lang="ts">
import { Play, Plus, RefreshCw, RotateCw, Search, Square, Trash2 } from 'lucide-vue-next'
import { computed, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerProject, Job } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { type Action, ActionMenu } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { Input } from '@/shared/ui/input'

import { useProjectAction, useProjects, useRedeploy, useUpdatable } from '../api'
import { deploymentOf, projectStatus, statusVariant } from '../lib/status'
import { DeployLogDialog } from '@/shared/ui/deploy-log'
import ProjectIcon from './ProjectIcon.vue'
import DeployingBadge from './DeployingBadge.vue'
import RedeployDialog from './RedeployDialog.vue'

const emit = defineEmits<{ open: [name: string]; create: [] }>()

const { t } = useI18n()
const { data: projects, isPending } = useProjects()
const act = useProjectAction()
const updatable = useUpdatable()
const filter = ref('')
const deleting = ref<DockerProject | null>(null)

const shown = computed(() => {
  const term = filter.value.trim().toLowerCase()
  return (projects.value ?? []).filter((p) => !term || p.name.includes(term))
})

function actions(project: DockerProject): Action[] {
  const app = project.kind === 'app'
  return [
    {
      id: 'restart',
      label: t('docker.actions.restart'),
      icon: RotateCw,
      disabled: !project.containers.length || busy.has(project.name),
    },
    { id: 'redeploy', label: t('docker.actions.redeploy'), icon: RefreshCw, disabled: app },
    {
      id: 'delete',
      label: t('docker.actions.delete'),
      icon: Trash2,
      destructive: true,
      disabled: app,
    },
  ]
}

function run(project: DockerProject, action: string): void {
  if (action === 'delete') {
    deleting.value = project
    return
  }
  if (action === 'redeploy') {
    asking.value = project.name
    return
  }
  void control(project.name, action as 'start' | 'stop' | 'restart')
}

/** Projects being started, stopped or restarted now: only their buttons wait. */
const busy = reactive(new Set<string>())
async function control(name: string, action: 'start' | 'stop' | 'restart'): Promise<void> {
  busy.add(name)
  try {
    await act.mutateAsync({ name, action })
  } catch {
    // Shown through the action's error.
  } finally {
    busy.delete(name)
  }
}

/** Redeploying asks first, then shows what Compose writes as it does. */
const redeploy = useRedeploy()
const jobs = useJobsStore()
const asking = ref<string | null>(null)
const deploying = ref<number | null>(null)
const showing = ref(false)
/** Opens the output of a deployment running now, closed before. */
function watchDeploy(id: number): void {
  deploying.value = id
  showing.value = true
}
function deploy(pull: boolean): void {
  const name = asking.value
  if (!name) return
  redeploy.mutate(
    { name, pull },
    {
      onSuccess: (job: Job) => {
        jobs.update(job)
        deploying.value = job.id
        asking.value = null
        showing.value = true
      },
    },
  )
}
function confirmDelete(): void {
  if (deleting.value) act.mutate({ name: deleting.value.name, action: 'delete' })
  deleting.value = null
}
</script>

<!-- The Compose projects, whoever made them. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <Button @click="emit('create')">
        <Plus />
        {{ t('docker.projects.create') }}
      </Button>
      <div class="relative ml-auto w-full max-w-60">
        <Search class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="filter"
          class="pl-8"
          :placeholder="t('docker.search')"
          :aria-label="t('docker.search')"
        />
      </div>
    </div>
    <Alert v-if="act.error.value || redeploy.error.value" variant="destructive">{{
      problemMessage(act.error.value ?? redeploy.error.value)
    }}</Alert>
    <div v-if="isPending" class="h-40 animate-pulse rounded-xl bg-muted" />
    <p v-else-if="!shown.length" class="py-16 text-center text-sm text-muted-foreground">
      {{ t('docker.projects.none') }}
    </p>
    <ul class="grid gap-3">
      <li
        v-for="project in shown"
        :key="project.name"
        class="flex cursor-pointer flex-wrap items-center gap-4 rounded-xl border bg-card p-4 transition-colors hover:bg-accent/40"
        @click="emit('open', project.name)"
      >
        <ProjectIcon :project="project" />
        <div class="grid min-w-40 flex-1 gap-1">
          <span class="truncate font-medium">{{ project.name }}</span>
          <span class="flex flex-wrap gap-1.5">
            <DeployingBadge
              v-if="deploymentOf(jobs.running, project.name)"
              @show="watchDeploy(deploymentOf(jobs.running, project.name)!.id)"
            />
            <Badge v-else :variant="statusVariant(projectStatus(project))">{{
              t(`docker.status.${projectStatus(project)}`)
            }}</Badge>
            <Badge v-if="project.kind !== 'own'" variant="outline">{{
              t(`docker.kinds.${project.kind}`)
            }}</Badge>
            <Badge
              v-if="project.kind !== 'app' && project.containers.some((c) => updatable.has(c.name))"
              variant="info"
              >{{ t('docker.updatable') }}</Badge
            >
          </span>
        </div>
        <div class="grid min-w-32 text-center">
          <span class="text-lg font-semibold tabular-nums">
            <span class="text-success">{{ project.running }}</span> /
            {{ project.containers.length }}
          </span>
          <span class="text-xs text-muted-foreground">{{ t('docker.projects.runningOfAll') }}</span>
        </div>
        <div class="flex items-center gap-1" @click.stop>
          <Button
            variant="ghost"
            size="sm"
            :disabled="
              !project.containers.length ||
              project.running === project.containers.length ||
              busy.has(project.name)
            "
            @click="run(project, 'start')"
          >
            <Play />
            {{ t('docker.actions.start') }}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            :disabled="project.running === 0 || busy.has(project.name)"
            @click="run(project, 'stop')"
          >
            <Square />
            {{ t('docker.actions.stop') }}
          </Button>
          <ActionMenu
            :actions="actions(project)"
            :label="t('docker.actions.more')"
            @select="run(project, $event)"
          />
        </div>
      </li>
    </ul>
    <ConfirmDialog
      :open="deleting !== null"
      :title="t('docker.projects.deleteTitle', { name: deleting?.name })"
      :description="t('docker.projects.deleteText')"
      :confirm-label="t('docker.actions.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (deleting = null)"
      @confirm="confirmDelete"
    />
    <RedeployDialog
      :open="asking !== null"
      :name="asking ?? ''"
      :busy="redeploy.isPending.value"
      @update:open="(open: boolean) => !open && (asking = null)"
      @confirm="deploy"
    />
    <DeployLogDialog v-model:open="showing" :job-id="deploying" />
  </div>
</template>
