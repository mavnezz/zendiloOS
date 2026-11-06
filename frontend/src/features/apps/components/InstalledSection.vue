<script setup lang="ts">
import { LoaderCircle, Play, RotateCcw, ScrollText, Square, Trash2 } from 'lucide-vue-next'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { InstalledApp } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import { type Action, ActionMenu } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { iconUrl, useInstallActions, useInstalled, webUrl } from '../api'
import { useAppUpdates } from '../lib/update'
import { appText } from '../texts'
import AppRow from './AppRow.vue'

const emit = defineEmits<{ open: [app: InstalledApp]; store: [] }>()

const { t, locale } = useI18n()
const { data: apps, error, isPending, refetch } = useInstalled()
const actions = useInstallActions()
const updates = useAppUpdates()
const notifications = useNotifications()
const jobs = useJobsStore()

const busy = ref<string | null>(null)
const logsOf = ref<InstalledApp | null>(null)
const logs = ref('')
const logsError = ref<string | null>(null)
const removing = ref<InstalledApp | null>(null)
const confirm = ref('')
const removeError = ref<string | null>(null)

// What Docker reports changes without a job: look again now and then.
const timer = window.setInterval(() => void refetch(), 15_000)
onBeforeUnmount(() => window.clearInterval(timer))
watch(
  () => jobs.list.filter((job) => job.kind.startsWith('apps.') && job.state !== 'running').length,
  () => void refetch(),
)

const updatable = computed(() =>
  (apps.value ?? []).filter((app) => app.update && app.state === 'installed'),
)
const sorted = computed(() =>
  [...(apps.value ?? [])].sort((a, b) =>
    appText(a, 'name', locale.value).localeCompare(appText(b, 'name', locale.value)),
  ),
)

/** The running job of an app, if any, for its step. */
const runningJobs = computed(
  () =>
    new Map(
      jobs.running.filter((job) => job.kind.startsWith('apps.')).map((job) => [job.target, job]),
    ),
)

function state(app: InstalledApp): string {
  if (app.state === 'installing') {
    const step = runningJobs.value.get(app.id)?.step
    return step ? t(`appStore.installed.steps.${step}`) : t('appStore.installed.states.installing')
  }
  if (app.state === 'removing' || app.state === 'failed')
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

function confirmUpdate(): void {
  const app = updates.asking.value
  updates.asking.value = null
  if (app) void updates.update(app, true)
}

const pill = 'h-7 rounded-full px-4 text-[13px] font-medium'
const heading = 'flex items-center gap-2 text-xl font-semibold'
const count =
  'rounded-full bg-foreground/[0.07] px-2 text-sm font-normal text-muted-foreground tabular-nums'
</script>

<!-- The apps on this NAS: those with a newer version first, then all of them. -->
<template>
  <div class="grid gap-6">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>

    <section v-if="updatable.length" class="grid">
      <div class="flex flex-wrap items-center gap-3">
        <h2 :class="heading">
          {{ t('appStore.installed.updatable') }} <span :class="count">{{ updatable.length }}</span>
        </h2>
        <button
          type="button"
          class="ml-auto text-sm font-medium text-primary hover:underline disabled:opacity-50"
          :disabled="updates.busy.value !== null"
          @click="updates.updateAll(updatable)"
        >
          {{ t('appStore.installed.updateAll') }}
        </button>
      </div>
      <ul class="grid gap-x-10 @2xl:grid-cols-2 @5xl:grid-cols-3">
        <li v-for="app in updatable" :key="app.id" class="border-b pb-3">
          <AppRow
            :name="appText(app, 'name', locale)"
            :subtitle="
              app.update?.version
                ? t('appStore.installed.newVersion', { version: app.update.version })
                : t('appStore.installed.newRelease')
            "
            :icon="iconUrl(app)"
            plain
            @open="emit('open', app)"
          >
            <Button
              variant="secondary"
              :class="pill"
              class="text-primary"
              :disabled="updates.busy.value === app.id"
              @click="updates.update(app)"
              >{{ t('appStore.installed.update') }}</Button
            >
          </AppRow>
          <p v-if="app.update?.version && app.version" class="text-sm">
            <span class="font-medium">{{ app.update.version }}</span>
            <span class="ml-2 text-muted-foreground">{{
              t('appStore.installed.fromVersion', { version: app.version })
            }}</span>
          </p>
        </li>
      </ul>
    </section>

    <section class="grid">
      <h2 :class="heading">
        {{ t('appStore.sections.installed') }} <span :class="count">{{ apps?.length ?? 0 }}</span>
      </h2>
      <div v-if="isPending" class="mt-4 h-40 animate-pulse rounded-xl bg-muted" />
      <div
        v-else-if="!apps?.length"
        class="grid justify-items-start gap-2 py-6 text-sm text-muted-foreground"
      >
        <p>{{ t('appStore.installed.empty') }}</p>
        <Button size="sm" variant="outline" @click="emit('store')">{{
          t('appStore.installed.toStore')
        }}</Button>
      </div>
      <ul class="grid gap-x-10 @2xl:grid-cols-2 @5xl:grid-cols-3">
        <li v-for="app in sorted" :key="app.id">
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
              v-else-if="app.state === 'failed'"
              variant="secondary"
              :class="pill"
              class="text-primary"
              :disabled="busy === app.id"
              @click="act(app, () => actions.retry(app.id))"
            >
              <RotateCcw />
              {{ t('appStore.installed.retry') }}
            </Button>
            <Button
              v-else-if="webUrl(app) && app.run_state !== 'stopped'"
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
              v-else-if="app.run_state === 'stopped'"
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

    <ConfirmDialog
      :open="updates.asking.value !== null"
      :title="t('appStore.installed.updatePermissionsTitle')"
      :description="t('appStore.installed.updatePermissionsHint')"
      :confirm-label="t('appStore.installed.update')"
      :cancel-label="t('desktop.cancel')"
      @update:open="(value: boolean) => !value && (updates.asking.value = null)"
      @confirm="confirmUpdate"
    />
  </div>
</template>
