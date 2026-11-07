<script setup lang="ts">
import { Boxes, LoaderCircle, Play, ScrollText, Square, Trash2 } from 'lucide-vue-next'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AppProject, InstalledApp } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import { useOpenApp } from '@/shared/lib/window-section'
import { type Action, ActionMenu } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { iconUrl, useAppProjects, useInstallActions, useInstalled, webUrl } from '../api'
import { appText } from '../texts'
import AppRow from './AppRow.vue'

const emit = defineEmits<{ open: [app: { source_id: number; id: string }]; store: [] }>()

const { t, locale } = useI18n()
const projects = useAppProjects()
const earlier = useInstalled()
const actions = useInstallActions()
const notifications = useNotifications()
const jobs = useJobsStore()
const openApp = useOpenApp()

const busy = ref<string | null>(null)
const logsOf = ref<InstalledApp | null>(null)
const logs = ref('')
const logsError = ref<string | null>(null)
const removing = ref<InstalledApp | null>(null)
const confirm = ref('')
const removeError = ref<string | null>(null)

// What Docker reports changes without a job: look again now and then.
const timer = window.setInterval(() => void actions.refresh(), 15_000)
onBeforeUnmount(() => window.clearInterval(timer))
watch(
  () =>
    jobs.list.filter(
      (job) =>
        (job.kind.startsWith('apps.') || job.kind.startsWith('containers.project')) &&
        job.state !== 'running',
    ).length,
  () => void actions.refresh(),
)

const error = computed(() => projects.error.value ?? earlier.error.value)
const pending = computed(() => projects.isPending.value || earlier.isPending.value)
const byName = <T extends { name: string; translations: AppProject['translations'] }>(list: T[]) =>
  [...list].sort((a, b) =>
    appText(a, 'name', locale.value).localeCompare(appText(b, 'name', locale.value)),
  )
const made = computed(() => byName(projects.data.value ?? []))
const before = computed(() => byName(earlier.data.value ?? []))
const count = computed(() => made.value.length + before.value.length)

/** The running job of an app installed the earlier way, if any, for its step. */
const runningJobs = computed(
  () =>
    new Map(
      jobs.running.filter((job) => job.kind.startsWith('apps.')).map((job) => [job.target, job]),
    ),
)

/** A project made from an app: what Docker says, and which project it is. */
function projectState(project: AppProject): string {
  const run = t(`appStore.installed.run.${project.run_state}`)
  return project.project === project.id ? run : `${run} · ${project.project}`
}

/** The icon of a project's app, while its catalog still has it. */
function projectIcon(project: AppProject): string | undefined {
  return project.digest ? iconUrl({ ...project, digest: project.digest }) : undefined
}

function state(app: InstalledApp): string {
  if (app.state === 'removing') {
    const step = runningJobs.value.get(app.id)?.step
    return step ? t(`appStore.installed.steps.${step}`) : t('appStore.installed.states.removing')
  }
  if (app.state === 'installing' || app.state === 'failed')
    return t(`appStore.installed.states.${app.state}`)
  return t(`appStore.installed.run.${app.run_state ?? 'unknown'}`)
}

function menu(app: InstalledApp): Action[] {
  const ready = app.state === 'installed'
  return [
    app.run_state === 'stopped'
      ? { id: 'start', label: t('appStore.installed.start'), icon: Play, disabled: !ready }
      : { id: 'stop', label: t('appStore.installed.stop'), icon: Square, disabled: !ready },
    { id: 'logs', label: t('appStore.installed.logs'), icon: ScrollText },
    {
      id: 'remove',
      label: t('appStore.installed.remove'),
      icon: Trash2,
      destructive: true,
      disabled: app.state === 'installing' || app.state === 'removing',
    },
  ]
}

async function act(app: InstalledApp, action: () => Promise<unknown>): Promise<void> {
  busy.value = app.id
  try {
    await action()
  } catch (problem) {
    notifications.notify('error', appText(app, 'name', locale.value), problemMessage(problem))
  } finally {
    busy.value = null
  }
}

function choose(app: InstalledApp, id: string): void {
  if (id === 'start') void act(app, () => actions.start(app.id))
  else if (id === 'stop') void act(app, () => actions.stop(app.id))
  else if (id === 'logs') void showLogs(app)
  else if (id === 'remove') {
    removing.value = app
    confirm.value = ''
    removeError.value = null
  }
}

async function showLogs(app: InstalledApp): Promise<void> {
  logsOf.value = app
  logs.value = ''
  logsError.value = null
  try {
    logs.value = (await actions.logs(app.id)).text
  } catch (problem) {
    logsError.value = problemMessage(problem)
  }
}

async function remove(): Promise<void> {
  const app = removing.value
  if (!app) return
  removeError.value = null
  try {
    await actions.uninstall(app.id, confirm.value)
    notifications.notify(
      'info',
      t('appStore.installed.removingNotice', { name: appText(app, 'name', locale.value) }),
    )
    removing.value = null
  } catch (problem) {
    removeError.value = problemMessage(problem)
  }
}

const pill = 'h-7 rounded-full px-4 text-[13px] font-medium'
const heading = 'flex items-center gap-2 text-xl font-semibold'
const counter =
  'rounded-full bg-foreground/[0.07] px-2 text-sm font-normal text-muted-foreground tabular-nums'
</script>

<!--
  The apps on this NAS: the Docker projects made from them, managed in the
  Docker app, and those installed the earlier way, which are only run and
  removed.
-->
<template>
  <div class="grid gap-6">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>

    <section class="grid">
      <h2 :class="heading">
        {{ t('appStore.sections.installed') }} <span :class="counter">{{ count }}</span>
      </h2>
      <div v-if="pending" class="mt-4 h-40 animate-pulse rounded-xl bg-muted" />
      <div
        v-else-if="!count"
        class="grid justify-items-start gap-2 py-6 text-sm text-muted-foreground"
      >
        <p>{{ t('appStore.installed.empty') }}</p>
        <Button size="sm" variant="outline" @click="emit('store')">{{
          t('appStore.installed.toStore')
        }}</Button>
      </div>
      <p v-else-if="made.length" class="mt-1 text-sm text-muted-foreground">
        {{ t('appStore.installed.projectsHint') }}
      </p>
      <ul class="grid gap-x-10 @2xl:grid-cols-2 @5xl:grid-cols-3">
        <li v-for="project in made" :key="project.project">
          <AppRow
            :name="appText(project, 'name', locale)"
            :subtitle="projectState(project)"
            :icon="projectIcon(project)"
            @open="emit('open', project)"
          >
            <Button
              v-if="webUrl(project)"
              as="a"
              variant="secondary"
              :class="pill"
              class="text-primary"
              :href="webUrl(project) ?? undefined"
              target="_blank"
              rel="noopener noreferrer"
              >{{ t('appStore.installed.open') }}</Button
            >
            <Button
              v-if="openApp"
              variant="ghost"
              size="icon"
              class="size-7"
              :aria-label="t('appStore.installed.inDocker')"
              :title="t('appStore.installed.inDocker')"
              @click="openApp('docker', `project:${project.project}`)"
            >
              <Boxes />
            </Button>
          </AppRow>
        </li>
      </ul>
    </section>

    <section v-if="before.length" class="grid">
      <h2 :class="heading">
        {{ t('appStore.installed.earlier') }} <span :class="counter">{{ before.length }}</span>
      </h2>
      <p class="mt-1 text-sm text-muted-foreground">{{ t('appStore.installed.earlierHint') }}</p>
      <ul class="grid gap-x-10 @2xl:grid-cols-2 @5xl:grid-cols-3">
        <li v-for="app in before" :key="app.id">
          <AppRow
            :name="appText(app, 'name', locale)"
            :subtitle="state(app)"
            :icon="iconUrl(app)"
            @open="emit('open', app)"
          >
            <LoaderCircle
              v-if="app.state === 'installing' || app.state === 'removing'"
              class="size-4 animate-spin text-muted-foreground"
            />
            <Button
              v-else-if="app.state === 'installed' && webUrl(app) && app.run_state !== 'stopped'"
              as="a"
              variant="secondary"
              :class="pill"
              class="text-primary"
              :href="webUrl(app) ?? undefined"
              target="_blank"
              rel="noopener noreferrer"
              >{{ t('appStore.installed.open') }}</Button
            >
            <Button
              v-else-if="app.state === 'installed' && app.run_state === 'stopped'"
              variant="secondary"
              :class="pill"
              class="text-primary"
              :disabled="busy === app.id"
              @click="act(app, () => actions.start(app.id))"
              >{{ t('appStore.installed.start') }}</Button
            >
            <ActionMenu
              :actions="menu(app)"
              :label="t('docker.actions.more')"
              @select="choose(app, $event)"
            />
          </AppRow>
        </li>
      </ul>
    </section>

    <ModalDialog
      :open="logsOf !== null"
      wide
      :title="
        t('appStore.installed.logsTitle', { name: logsOf ? appText(logsOf, 'name', locale) : '' })
      "
      :close-label="t('desktop.close')"
      @update:open="(value: boolean) => !value && (logsOf = null)"
    >
      <Alert v-if="logsError" variant="destructive">{{ logsError }}</Alert>
      <pre
        v-else
        class="max-h-[60dvh] overflow-auto rounded-md bg-zinc-900 p-3 font-mono text-xs whitespace-pre-wrap text-zinc-100"
        >{{ logs || t('appStore.installed.noLogs') }}</pre>
      <div class="flex justify-end">
        <Button v-if="logsOf" size="sm" variant="outline" @click="showLogs(logsOf)">
          {{ t('appStore.installed.reload') }}
        </Button>
      </div>
    </ModalDialog>

    <ModalDialog
      :open="removing !== null"
      :title="
        t('appStore.installed.removeTitle', {
          name: removing ? appText(removing, 'name', locale) : '',
        })
      "
      :description="t('appStore.installed.removeHint')"
      :close-label="t('desktop.close')"
      @update:open="(value: boolean) => !value && (removing = null)"
    >
      <form v-if="removing" class="grid gap-3" @submit.prevent="remove">
        <Label for="remove-confirm">{{
          t('appStore.installed.removeConfirm', { id: removing.id })
        }}</Label>
        <Input id="remove-confirm" v-model="confirm" autocomplete="off" />
        <Alert v-if="removeError" variant="destructive">{{ removeError }}</Alert>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" @click="removing = null">{{
            t('desktop.cancel')
          }}</Button>
          <Button type="submit" variant="destructive" :disabled="confirm.trim() !== removing.id">
            {{ t('appStore.installed.remove') }}
          </Button>
        </div>
      </form>
    </ModalDialog>
  </div>
</template>
