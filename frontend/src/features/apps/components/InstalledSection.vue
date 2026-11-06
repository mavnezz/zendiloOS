<script setup lang="ts">
import {
  ArrowUpCircle,
  Blocks,
  ExternalLink,
  Play,
  RotateCcw,
  ScrollText,
  Square,
  Trash2,
} from 'lucide-vue-next'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { ApiProblem } from '@/shared/api/client'
import type { InstalledApp } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { iconUrl, useInstallActions, useInstalled, webUrl } from '../api'
import { appText } from '../texts'

defineEmits<{ store: [] }>()

const { t, locale } = useI18n()
const { data: apps, error, isPending, refetch } = useInstalled()
const actions = useInstallActions()
const notifications = useNotifications()
const jobs = useJobsStore()

const busy = ref<string | null>(null)
const failure = ref<string | null>(null)
const missingIcons = ref(new Set<string>())

const logsOf = ref<InstalledApp | null>(null)
const logs = ref('')
const logsError = ref<string | null>(null)
const removing = ref<InstalledApp | null>(null)
const confirm = ref('')
const removeError = ref<string | null>(null)
const updating = ref<InstalledApp | null>(null)

// What Docker reports changes without a job: look again now and then.
const timer = window.setInterval(() => void refetch(), 15_000)
onBeforeUnmount(() => window.clearInterval(timer))

watch(
  () => jobs.list.filter((job) => job.kind.startsWith('apps.') && job.state !== 'running').length,
  () => void refetch(),
)

/** The running job of an app, if any, for its step. */
const runningJobs = computed(
  () =>
    new Map(
      jobs.running.filter((job) => job.kind.startsWith('apps.')).map((job) => [job.target, job]),
    ),
)

function stateLabel(app: InstalledApp): { text: string; tone: 'ok' | 'warn' | 'bad' | 'muted' } {
  if (app.state === 'installing') {
    const step = runningJobs.value.get(app.id)?.step
    const text = step
      ? t(`appStore.installed.steps.${step}`)
      : t('appStore.installed.states.installing')
    return { text, tone: 'muted' }
  }
  if (app.state === 'removing')
    return { text: t('appStore.installed.states.removing'), tone: 'muted' }
  if (app.state === 'failed') return { text: t('appStore.installed.states.failed'), tone: 'bad' }
  switch (app.run_state) {
    case 'running':
      return { text: t('appStore.installed.run.running'), tone: 'ok' }
    case 'partly':
      return { text: t('appStore.installed.run.partly'), tone: 'warn' }
    case 'restarting':
      return { text: t('appStore.installed.run.restarting'), tone: 'bad' }
    case 'stopped':
      return { text: t('appStore.installed.run.stopped'), tone: 'muted' }
    default:
      return { text: t('appStore.installed.run.unknown'), tone: 'muted' }
  }
}

const tones = {
  ok: 'border-emerald-500/60 text-emerald-700 dark:text-emerald-400',
  warn: 'border-amber-500/60 text-amber-700 dark:text-amber-400',
  bad: 'border-destructive/60 text-destructive',
  muted: '',
}

async function act(app: InstalledApp, action: () => Promise<unknown>): Promise<void> {
  busy.value = app.id
  failure.value = null
  try {
    await action()
  } catch (problem) {
    failure.value = `${appText(app, 'name', locale.value)}: ${problemMessage(problem)}`
  } finally {
    busy.value = null
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

async function update(app: InstalledApp, confirmPermissions = false): Promise<void> {
  busy.value = app.id
  failure.value = null
  try {
    await actions.update(app.id, confirmPermissions)
    notifications.notify(
      'info',
      t('appStore.installed.updating', { name: appText(app, 'name', locale.value) }),
    )
  } catch (problem) {
    if (problem instanceof ApiProblem && problem.code === 'permissions_not_confirmed') {
      updating.value = app
    } else {
      failure.value = `${appText(app, 'name', locale.value)}: ${problemMessage(problem)}`
    }
  } finally {
    busy.value = null
  }
}

function askRemove(app: InstalledApp): void {
  removing.value = app
  confirm.value = ''
  removeError.value = null
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
</script>

<!-- The apps on this NAS: whether they run, and what can be done with them. -->
<template>
  <div class="grid gap-4">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <div
      v-if="!isPending && !apps?.length"
      class="grid justify-items-start gap-2 text-sm text-muted-foreground"
    >
      <p>{{ t('appStore.installed.empty') }}</p>
      <Button size="sm" variant="outline" @click="$emit('store')">{{
        t('appStore.installed.toStore')
      }}</Button>
    </div>

    <ul class="grid gap-3">
      <li v-for="app in apps" :key="app.id" class="grid gap-3 rounded-xl border bg-card p-3">
        <div class="flex items-start gap-3">
          <Blocks
            v-if="missingIcons.has(app.id)"
            class="size-12 shrink-0 rounded-lg text-muted-foreground"
          />
          <img
            v-else
            :src="iconUrl(app)"
            alt=""
            class="size-12 shrink-0 rounded-lg"
            @error="missingIcons = new Set([...missingIcons, app.id])"
          />
          <div class="grid min-w-0 flex-1 gap-1">
            <p class="flex flex-wrap items-center gap-2">
              <span class="truncate font-medium">{{ appText(app, 'name', locale) }}</span>
              <span class="text-xs text-muted-foreground">{{ app.version }}</span>
              <Badge variant="outline" :class="tones[stateLabel(app).tone]">{{
                stateLabel(app).text
              }}</Badge>
              <Badge v-if="app.update" variant="secondary">
                {{ t('appStore.installed.updateAvailable', { version: app.update }) }}
              </Badge>
            </p>
            <p class="line-clamp-1 text-xs text-muted-foreground">
              {{ appText(app, 'tagline', locale) }}
            </p>
            <p class="text-xs text-muted-foreground">
              <span v-for="(port, id) in app.ports" :key="id" class="mr-3">{{
                t('appStore.installed.port', { id, port })
              }}</span>
              <span v-for="(share, id) in app.shares" :key="id" class="mr-3">{{
                t('appStore.installed.share', { id, share })
              }}</span>
            </p>
          </div>
        </div>
        <div class="flex flex-wrap gap-2">
          <Button
            v-if="webUrl(app) && app.state === 'installed'"
            as="a"
            size="sm"
            :href="webUrl(app) ?? undefined"
            target="_blank"
            rel="noopener noreferrer"
          >
            <ExternalLink />
            {{ t('appStore.installed.open') }}
          </Button>
          <template v-if="app.state === 'installed'">
            <Button
              v-if="app.run_state === 'stopped'"
              size="sm"
              variant="outline"
              :disabled="busy === app.id"
              @click="act(app, () => actions.start(app.id))"
            >
              <Play />
              {{ t('appStore.installed.start') }}
            </Button>
            <Button
              v-else
              size="sm"
              variant="outline"
              :disabled="busy === app.id"
              @click="act(app, () => actions.stop(app.id))"
            >
              <Square />
              {{ t('appStore.installed.stop') }}
            </Button>
            <Button
              v-if="app.update"
              size="sm"
              variant="outline"
              :disabled="busy === app.id"
              @click="update(app)"
            >
              <ArrowUpCircle />
              {{ t('appStore.installed.update') }}
            </Button>
          </template>
          <Button
            v-if="app.state === 'failed'"
            size="sm"
            variant="outline"
            :disabled="busy === app.id"
            @click="act(app, () => actions.retry(app.id))"
          >
            <RotateCcw />
            {{ t('appStore.installed.retry') }}
          </Button>
          <Button size="sm" variant="ghost" @click="showLogs(app)">
            <ScrollText />
            {{ t('appStore.installed.logs') }}
          </Button>
          <Button
            v-if="app.state === 'installed' || app.state === 'failed'"
            size="sm"
            variant="ghost"
            class="text-destructive"
            @click="askRemove(app)"
          >
            <Trash2 />
            {{ t('appStore.installed.remove') }}
          </Button>
        </div>
      </li>
    </ul>

    <ModalDialog
      :open="logsOf !== null"
      wide
      :title="
        t('appStore.installed.logsTitle', { name: logsOf ? appText(logsOf, 'name', locale) : '' })
      "
      :close-label="t('desktop.close')"
      @update:open="(value) => !value && (logsOf = null)"
    >
      <Alert v-if="logsError" variant="destructive">{{ logsError }}</Alert>
      <pre
        v-else
        class="max-h-[60dvh] overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs whitespace-pre-wrap"
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
      @update:open="(value) => !value && (removing = null)"
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

    <ConfirmDialog
      :open="updating !== null"
      :title="t('appStore.installed.updatePermissionsTitle')"
      :description="t('appStore.installed.updatePermissionsHint')"
      :confirm-label="t('appStore.installed.update')"
      :cancel-label="t('desktop.cancel')"
      @update:open="(value) => !value && (updating = null)"
      @confirm="updating && update(updating, true)"
    />
  </div>
</template>
