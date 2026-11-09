<script setup lang="ts">
import {
  CircleCheck,
  LoaderCircle,
  PackageCheck,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  TriangleAlert,
} from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { useUpdateActions, useUpdates } from '@/features/updates/api'
import type { UpdateSettings } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { Markdown } from '@/shared/ui/markdown'
import { PageCard, PageFooter, PageHeader, PageTabs, SettingRow } from '@/shared/ui/page'
import { RadioGroup, RadioGroupItem } from '@/shared/ui/radio-group'
import { Switch } from '@/shared/ui/switch'

import { usePower } from '../../power'

const { t } = useI18n()
const { data: overview, error, busy } = useUpdates()
const actions = useUpdateActions()
const power = usePower()

const tab = ref('update')
const failure = ref<string | null>(null)
const asking = ref<'update' | 'restart' | null>(null)
const showPackages = ref(false)
const showNotes = ref(false)

/** The version this page was loaded with; another one means zendiloOS was updated. */
const loadedWith = ref<string | null>(null)
watch(
  () => overview.value?.current,
  (current) => {
    if (current && loadedWith.value === null) loadedWith.value = current
  },
  { immediate: true },
)
const updatedMeanwhile = computed(
  () => loadedWith.value !== null && overview.value?.current !== loadedWith.value,
)

const run = computed(() => overview.value?.run ?? null)
const running = computed(() => run.value?.state === 'running')
const hasUpdates = computed(
  () => Boolean(overview.value?.release) || (overview.value?.packages.length ?? 0) > 0,
)
const security = computed(() => overview.value?.packages.filter((p) => p.security).length ?? 0)
const newKernel = computed(() =>
  overview.value?.packages.some((p) => p.name.startsWith('linux-image')),
)
/** The steps of the run, in order, the release only when one is installed. */
const steps = computed(() => [
  'lists',
  'debian',
  ...(run.value?.zendiloos ? ['zendiloos'] : []),
  'modules',
])
function stepState(step: string): 'done' | 'now' | 'later' {
  const at = steps.value.indexOf(run.value?.step ?? '')
  const index = steps.value.indexOf(step)
  if (run.value?.state === 'succeeded' || index < at) return 'done'
  return index === at ? 'now' : 'later'
}

async function act(what: () => Promise<unknown>): Promise<void> {
  failure.value = null
  try {
    await what()
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

function confirmed(): void {
  const what = asking.value
  asking.value = null
  if (what === 'update') void act(actions.apply)
  if (what === 'restart') void act(() => power.request('reboot'))
}

// Settings: a draft, applied with the page's footer.
const draft = ref<UpdateSettings | null>(null)
watch(
  () => overview.value?.settings,
  (settings) => {
    if (settings && !draft.value) draft.value = { ...settings }
  },
  { immediate: true },
)
const dirty = computed(
  () =>
    Boolean(draft.value && overview.value) &&
    (draft.value!.channel !== overview.value!.settings.channel ||
      draft.value!.check_daily !== overview.value!.settings.check_daily),
)
const saving = ref(false)
const saveError = ref<string | null>(null)
async function save(): Promise<void> {
  if (!draft.value) return
  saving.value = true
  saveError.value = null
  try {
    draft.value = { ...(await actions.save(draft.value)) }
  } catch (problem) {
    saveError.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}
function reset(): void {
  if (overview.value) draft.value = { ...overview.value.settings }
  saveError.value = null
}

function reload(): void {
  window.location.reload()
}

const when = (seconds: number | null | undefined) =>
  seconds ? formatDateTime(seconds * 1000) : '–'
const cell = 'px-3 py-1.5 text-left'
</script>

<!-- Keeping zendiloOS and Debian current: look, read what is new, install. -->
<template>
  <div>
    <PageHeader :title="t('controlPanel.update')" :description="t('updates.description')" />
    <PageTabs
      v-model="tab"
      :tabs="[
        { id: 'update', label: t('updates.tabUpdate') },
        { id: 'settings', label: t('updates.tabSettings') },
      ]"
      :label="t('controlPanel.update')"
    />
    <Alert v-if="error && !overview" variant="destructive" class="mb-4">{{
      problemMessage(error)
    }}</Alert>

    <div v-if="tab === 'update' && overview" class="grid max-w-3xl gap-4">
      <Alert v-if="updatedMeanwhile">
        <PackageCheck />
        <span class="flex flex-wrap items-center gap-3">
          {{ t('updates.reloadHint', { version: overview.current }) }}
          <Button size="sm" variant="outline" @click="reload">{{ t('updates.reload') }}</Button>
        </span>
      </Alert>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      <Alert v-if="overview.phase === 'failed' && overview.phase_error" variant="destructive">
        {{ overview.phase_error }}
      </Alert>

      <PageCard :title="t('updates.system')" :icon="RefreshCw">
        <template #actions>
          <Button variant="outline" size="sm" :disabled="busy" @click="act(actions.check)">
            <LoaderCircle v-if="overview.phase === 'checking'" class="animate-spin" />
            <RefreshCw v-else />
            {{ overview.phase === 'checking' ? t('updates.checking') : t('updates.check') }}
          </Button>
        </template>
        <dl class="grid gap-x-6 gap-y-2 sm:grid-cols-[11rem_1fr]">
          <dt class="text-muted-foreground">{{ t('updates.version') }}</dt>
          <dd class="font-medium">zendiloOS {{ overview.current }}</dd>
          <dt class="text-muted-foreground">{{ t('updates.channel') }}</dt>
          <dd>{{ t(`updates.channels.${overview.settings.channel}`) }}</dd>
          <dt class="text-muted-foreground">{{ t('updates.kernel') }}</dt>
          <dd class="font-mono text-[13px]">{{ overview.running_kernel ?? '–' }}</dd>
          <dt class="text-muted-foreground">{{ t('updates.checkedAt') }}</dt>
          <dd>{{ when(overview.checked_at) }}</dd>
        </dl>
      </PageCard>

      <PageCard v-if="run" :title="t('updates.runTitle')" :icon="PackageCheck">
        <ol class="grid gap-2">
          <li v-for="step in steps" :key="step" class="flex items-center gap-2">
            <CircleCheck v-if="stepState(step) === 'done'" class="size-4 text-emerald-600" />
            <LoaderCircle
              v-else-if="stepState(step) === 'now' && running"
              class="size-4 animate-spin text-primary"
            />
            <TriangleAlert v-else-if="stepState(step) === 'now'" class="size-4 text-destructive" />
            <span v-else class="size-4 rounded-full border" />
            <span :class="stepState(step) === 'later' && 'text-muted-foreground'">{{
              t(`updates.steps.${step}`, { version: run.zendiloos ?? '' })
            }}</span>
          </li>
        </ol>
        <p v-if="running" class="mt-3 text-xs text-muted-foreground">{{ t('updates.runHint') }}</p>
        <template v-else-if="run.state === 'succeeded'">
          <Alert v-if="!run.zfs_ready" variant="destructive" class="mt-3">
            <ShieldAlert />
            {{ t('updates.zfsMissing') }}
          </Alert>
          <div v-else class="mt-3 flex flex-wrap items-center gap-3">
            <p class="flex items-center gap-1.5 font-medium text-emerald-700 dark:text-emerald-400">
              <CircleCheck class="size-4" />
              {{ t('updates.done', { at: when(run.finished_at) }) }}
            </p>
            <Button
              v-if="overview.restart_needed"
              size="sm"
              class="ml-auto"
              @click="asking = 'restart'"
            >
              <RotateCcw />
              {{ t('updates.restartNow') }}
            </Button>
          </div>
        </template>
        <div v-else class="mt-3 grid gap-2">
          <Alert variant="destructive">{{ t('updates.failed') }}</Alert>
          <pre
            v-if="run.error"
            class="max-h-60 overflow-auto rounded-md bg-zinc-900 p-3 font-mono text-xs whitespace-pre-wrap text-zinc-100"
            >{{ run.error }}</pre>
        </div>
      </PageCard>

      <PageCard v-if="!running && hasUpdates" :title="t('updates.available')" :icon="PackageCheck">
        <template #actions>
          <Button size="sm" :disabled="busy" @click="asking = 'update'">
            <LoaderCircle v-if="overview.phase === 'downloading'" class="animate-spin" />
            {{
              overview.phase === 'downloading' ? t('updates.downloading') : t('updates.updateNow')
            }}
          </Button>
        </template>
        <div class="grid gap-4">
          <section v-if="overview.release" class="grid gap-1.5">
            <h4 class="flex flex-wrap items-center gap-2 font-medium">
              zendiloOS {{ overview.release.version }}
              <Badge v-if="overview.release.channel !== 'stable'" variant="warning">{{
                t(`updates.channels.${overview.release.channel}`)
              }}</Badge>
            </h4>
            <p class="text-xs text-muted-foreground">
              {{ t('updates.published', { at: when(overview.release.published_at) }) }} ·
              {{ formatBytes(overview.release.size) }}
            </p>
            <button
              v-if="overview.release.notes"
              type="button"
              class="w-fit text-left text-primary hover:underline"
              @click="showNotes = !showNotes"
            >
              {{ showNotes ? t('updates.hideNotes') : t('updates.showNotes') }}
            </button>
            <Markdown
              v-if="showNotes"
              :source="overview.release.notes"
              class="max-h-72 overflow-auto rounded-md bg-muted/50 p-3"
            />
          </section>
          <section v-if="overview.packages.length" class="grid gap-1.5">
            <h4 class="font-medium">
              {{
                t('updates.packages', { count: overview.packages.length }, overview.packages.length)
              }}
              <span v-if="security" class="ml-1 font-normal text-muted-foreground">{{
                t('updates.security', { count: security }, security)
              }}</span>
            </h4>
            <p v-if="newKernel" class="text-xs text-muted-foreground">
              {{ t('updates.kernelHint') }}
            </p>
            <button
              type="button"
              class="w-fit text-left text-primary hover:underline"
              @click="showPackages = !showPackages"
            >
              {{ showPackages ? t('updates.hidePackages') : t('updates.showPackages') }}
            </button>
            <div v-if="showPackages" class="max-h-80 overflow-auto rounded-md border">
              <table class="w-full text-[13px]">
                <thead class="sticky top-0 bg-card text-xs text-muted-foreground">
                  <tr class="border-b">
                    <th :class="cell" class="font-normal">{{ t('updates.package') }}</th>
                    <th :class="cell" class="font-normal">{{ t('updates.installed') }}</th>
                    <th :class="cell" class="font-normal">{{ t('updates.new') }}</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="p in overview.packages" :key="p.name" class="border-b last:border-0">
                    <td :class="cell">
                      {{ p.name }}
                      <Badge v-if="p.security" variant="destructive" class="ml-1">{{
                        t('updates.securityBadge')
                      }}</Badge>
                    </td>
                    <td :class="cell" class="font-mono text-muted-foreground">
                      {{ p.from ?? '–' }}
                    </td>
                    <td :class="cell" class="font-mono">{{ p.to }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>
        </div>
      </PageCard>

      <PageCard
        v-else-if="!running && overview.checked_at && !busy"
        :icon="CircleCheck"
        :title="t('updates.upToDate')"
      >
        <p class="text-muted-foreground">{{ t('updates.upToDateHint') }}</p>
      </PageCard>

      <Alert v-if="overview.release_error">
        <TriangleAlert />
        {{ t('updates.releaseError', { reason: overview.release_error }) }}
      </Alert>
      <Alert v-if="overview.packages_error">
        <TriangleAlert />
        {{ t('updates.packagesError', { reason: overview.packages_error }) }}
      </Alert>
      <Alert
        v-if="overview.restart_needed && !run"
        :variant="overview.zfs_ready ? 'default' : 'destructive'"
      >
        <RotateCcw />
        <span class="flex flex-wrap items-center gap-3">
          {{
            overview.zfs_ready
              ? t('updates.kernelWaits', { kernel: overview.newest_kernel })
              : t('updates.zfsMissing')
          }}
          <Button
            v-if="overview.zfs_ready"
            size="sm"
            variant="outline"
            @click="asking = 'restart'"
            >{{ t('updates.restartNow') }}</Button
          >
        </span>
      </Alert>
    </div>

    <div v-else-if="tab === 'settings' && draft" class="grid max-w-3xl gap-4">
      <PageCard :title="t('updates.settingsTitle')" :icon="RefreshCw">
        <div class="grid gap-5">
          <SettingRow :label="t('updates.channel')">
            <RadioGroup v-model="draft.channel" :label="t('updates.channel')">
              <RadioGroupItem
                v-for="c in ['stable', 'test', 'development'] as const"
                :key="c"
                :value="c"
              >
                <span class="grid gap-0.5">
                  <span>{{ t(`updates.channels.${c}`) }}</span>
                  <span class="text-xs text-muted-foreground">{{
                    t(`updates.channelHints.${c}`)
                  }}</span>
                </span>
              </RadioGroupItem>
            </RadioGroup>
            <Alert v-if="draft.channel === 'development'" class="mt-1">
              <TriangleAlert />
              {{ t('updates.developmentWarning') }}
            </Alert>
          </SettingRow>
          <SettingRow :label="t('updates.automatic')" for="updates-daily">
            <label class="flex items-center gap-2">
              <Switch id="updates-daily" v-model="draft.check_daily" />
              {{ t('updates.checkDaily') }}
            </label>
            <p class="text-xs text-muted-foreground">{{ t('updates.checkDailyHint') }}</p>
          </SettingRow>
        </div>
      </PageCard>
      <PageFooter :dirty="dirty" :saving="saving" :error="saveError" @apply="save" @reset="reset" />
    </div>

    <ConfirmDialog
      :open="asking !== null"
      :title="asking === 'restart' ? t('desktop.restartTitle') : t('updates.confirmTitle')"
      :description="
        asking === 'restart' ? t('desktop.restartDescription') : t('updates.confirmHint')
      "
      :confirm-label="asking === 'restart' ? t('desktop.restartAction') : t('updates.updateNow')"
      :cancel-label="t('desktop.cancel')"
      @update:open="(value: boolean) => !value && (asking = null)"
      @confirm="confirmed"
    />
  </div>
</template>
