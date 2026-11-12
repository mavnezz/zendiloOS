<script setup lang="ts">
import { ArchiveRestore, CalendarClock, Download } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { ConfigurationPlan, ConfigurationTarget, RestoreResult } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { waitForServer } from '@/shared/lib/passphrase'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'
import { PageCard, PageHeader, SettingRow } from '@/shared/ui/page'
import { Switch } from '@/shared/ui/switch'

import {
  downloadBackup,
  restoreBackup,
  runConfigurationBackup,
  useBackupPlaces,
  useConfigurationPlan,
  useSetConfigurationPlan,
} from '../api'

const { t } = useI18n()
const { data: plan } = useConfigurationPlan()
const save = useSetConfigurationPlan()
const { shares, drives } = useBackupPlaces()
const min = computed(() => plan.value?.passphrase_min ?? 12)

// Back up now.
const downloadPassphrase = ref('')
const downloadRepeat = ref('')
const downloading = ref(false)
const downloadError = ref<string | null>(null)
const downloadDiffers = computed(
  () => downloadRepeat.value !== '' && downloadPassphrase.value !== downloadRepeat.value,
)
async function download(): Promise<void> {
  if (downloadPassphrase.value !== downloadRepeat.value) return
  downloading.value = true
  downloadError.value = null
  try {
    await downloadBackup(downloadPassphrase.value)
    downloadPassphrase.value = ''
    downloadRepeat.value = ''
  } catch (failure) {
    downloadError.value = problemMessage(failure)
  } finally {
    downloading.value = false
  }
}

// The plan, as a draft until saved.
interface Draft {
  enabled: boolean
  frequency: 'daily' | 'weekly'
  weekday: number
  time: string
  kind: 'share' | 'drive'
  share: number | ''
  drive: string
  folder: string
  keep: number
  passphrase: string
  repeat: string
}
const pad = (value: number) => String(value).padStart(2, '0')
function draftOf(value: ConfigurationPlan): Draft {
  const target = value.target
  return {
    enabled: value.enabled,
    frequency: value.frequency,
    weekday: value.weekday ?? 0,
    time: `${pad(value.hour)}:${pad(value.minute)}`,
    kind: target?.kind ?? 'share',
    share: target?.kind === 'share' ? target.share_id : '',
    drive: target?.kind === 'drive' ? target.drive : '',
    folder: target?.folder ?? 'zendiloOS',
    keep: value.keep,
    passphrase: '',
    repeat: '',
  }
}
const draft = ref<Draft | null>(null)
watch(
  plan,
  (value) => {
    if (value && !draft.value) draft.value = draftOf(value)
  },
  { immediate: true },
)
const planDiffers = computed(
  () => !!draft.value && draft.value.repeat !== '' && draft.value.passphrase !== draft.value.repeat,
)
const planMessage = ref<string | null>(null)
function target(value: Draft): ConfigurationTarget | null {
  if (value.kind === 'share' && value.share !== '') {
    return { kind: 'share', share_id: Number(value.share), folder: value.folder.trim() }
  }
  if (value.kind === 'drive' && value.drive) {
    return { kind: 'drive', drive: value.drive, folder: value.folder.trim() }
  }
  return null
}
function savePlan(): void {
  const value = draft.value
  if (!value || planDiffers.value) return
  const [hour, minute] = value.time.split(':').map(Number)
  planMessage.value = null
  save.mutate(
    {
      enabled: value.enabled,
      frequency: value.frequency,
      weekday: value.frequency === 'weekly' ? value.weekday : null,
      hour: hour ?? 3,
      minute: minute ?? 0,
      target: target(value),
      keep: value.keep,
      passphrase: value.passphrase || null,
    },
    {
      onSuccess: (saved) => {
        draft.value = draftOf(saved)
      },
    },
  )
}
async function runNow(): Promise<void> {
  planMessage.value = null
  try {
    await runConfigurationBackup()
    planMessage.value = t('configurationBackup.plan.started')
  } catch (failure) {
    planMessage.value = problemMessage(failure)
  }
}
const last = computed(() => plan.value?.last_run)

// Restore.
const backupFile = ref<File | null>(null)
const restorePassphrase = ref('')
const confirming = ref(false)
const restoring = ref(false)
const restoreError = ref<string | null>(null)
const restored = ref<RestoreResult | null>(null)
function chosen(event: Event): void {
  backupFile.value = (event.target as HTMLInputElement).files?.[0] ?? null
}
async function restore(): Promise<void> {
  if (!backupFile.value) return
  restoring.value = true
  restoreError.value = null
  try {
    restored.value = await restoreBackup(backupFile.value, restorePassphrase.value)
    restorePassphrase.value = ''
    if (await waitForServer()) window.location.reload()
  } catch (failure) {
    restoreError.value = problemMessage(failure)
  } finally {
    restoring.value = false
  }
}
</script>

<!--
  The configuration in one encrypted file: downloaded now, written on a
  schedule into a folder, and put back from one.
-->
<template>
  <div class="grid gap-4">
    <PageHeader
      :title="t('controlPanel.configurationBackup')"
      :description="t('configurationBackup.description')"
    />

    <PageCard :title="t('configurationBackup.download.title')" :icon="Download">
      <form class="grid gap-4" @submit.prevent="download">
        <p class="text-sm text-muted-foreground">{{ t('configurationBackup.download.intro') }}</p>
        <Alert v-if="downloadError" variant="destructive">{{ downloadError }}</Alert>
        <SettingRow :label="t('configurationBackup.passphrase')" for="download-passphrase">
          <Input
            id="download-passphrase"
            v-model="downloadPassphrase"
            type="password"
            autocomplete="new-password"
            :minlength="min"
            required
          />
          <p class="text-xs text-muted-foreground">
            {{ t('configurationBackup.passphraseHint', { min }) }}
          </p>
        </SettingRow>
        <SettingRow :label="t('configurationBackup.passphraseRepeat')" for="download-repeat">
          <Input
            id="download-repeat"
            v-model="downloadRepeat"
            type="password"
            autocomplete="new-password"
            :aria-invalid="downloadDiffers"
            required
          />
          <p v-if="downloadDiffers" class="text-xs text-destructive">
            {{ t('configurationBackup.passphrasesDiffer') }}
          </p>
        </SettingRow>
        <div class="flex justify-end">
          <Button
            type="submit"
            data-testid="download"
            :disabled="downloading || downloadDiffers || downloadPassphrase.length < min"
          >
            {{ t('configurationBackup.download.submit') }}
          </Button>
        </div>
      </form>
    </PageCard>

    <PageCard
      v-if="draft && plan"
      :title="t('configurationBackup.plan.title')"
      :icon="CalendarClock"
    >
      <form class="grid gap-4" @submit.prevent="savePlan">
        <p class="text-sm text-muted-foreground">{{ t('configurationBackup.plan.intro') }}</p>
        <Alert v-if="save.error.value" variant="destructive">
          {{ problemMessage(save.error.value) }}
        </Alert>
        <SettingRow :label="t('configurationBackup.plan.enabled')" for="plan-enabled">
          <Switch id="plan-enabled" v-model="draft.enabled" />
        </SettingRow>
        <SettingRow :label="t('configurationBackup.plan.when')" for="plan-frequency">
          <div class="flex flex-wrap items-center gap-2">
            <NativeSelect id="plan-frequency" v-model="draft.frequency" class="w-36">
              <option value="daily">{{ t('configurationBackup.plan.daily') }}</option>
              <option value="weekly">{{ t('configurationBackup.plan.weekly') }}</option>
            </NativeSelect>
            <NativeSelect
              v-if="draft.frequency === 'weekly'"
              id="plan-weekday"
              v-model="draft.weekday"
              class="w-36"
            >
              <option v-for="day in 7" :key="day" :value="day - 1">
                {{ t(`configurationBackup.plan.weekdays.${day - 1}`) }}
              </option>
            </NativeSelect>
            <span class="text-sm text-muted-foreground">{{
              t('configurationBackup.plan.at')
            }}</span>
            <Input id="plan-time" v-model="draft.time" type="time" class="w-28" required />
          </div>
        </SettingRow>
        <SettingRow :label="t('configurationBackup.plan.where')" for="plan-kind">
          <div class="flex flex-wrap gap-2">
            <NativeSelect id="plan-kind" v-model="draft.kind" class="w-40">
              <option value="share">{{ t('configurationBackup.plan.share') }}</option>
              <option value="drive">{{ t('configurationBackup.plan.drive') }}</option>
            </NativeSelect>
            <NativeSelect
              v-if="draft.kind === 'share'"
              id="plan-share"
              v-model="draft.share"
              class="min-w-40"
            >
              <option value="">{{ t('configurationBackup.plan.choose') }}</option>
              <option v-for="share in shares.data.value ?? []" :key="share.id" :value="share.id">
                {{ share.name }}
              </option>
            </NativeSelect>
            <NativeSelect v-else id="plan-drive" v-model="draft.drive" class="min-w-40">
              <option value="">{{ t('configurationBackup.plan.choose') }}</option>
              <option v-for="drive in drives.data.value ?? []" :key="drive.id" :value="drive.id">
                {{ drive.label || drive.id }}
              </option>
            </NativeSelect>
          </div>
        </SettingRow>
        <SettingRow :label="t('configurationBackup.plan.folder')" for="plan-folder">
          <Input id="plan-folder" v-model="draft.folder" spellcheck="false" />
          <p class="text-xs text-muted-foreground">
            {{ t('configurationBackup.plan.folderHint') }}
          </p>
        </SettingRow>
        <SettingRow :label="t('configurationBackup.plan.keep')" for="plan-keep">
          <Input
            id="plan-keep"
            v-model.number="draft.keep"
            type="number"
            min="1"
            :max="plan.keep_max"
            class="w-24"
          />
          <p class="text-xs text-muted-foreground">
            {{ t('configurationBackup.plan.keepHint', { count: draft.keep }) }}
          </p>
        </SettingRow>
        <SettingRow :label="t('configurationBackup.passphrase')" for="plan-passphrase">
          <Input
            id="plan-passphrase"
            v-model="draft.passphrase"
            type="password"
            autocomplete="new-password"
          />
          <p class="text-xs text-muted-foreground">
            {{
              plan.passphrase_set
                ? t('configurationBackup.passphraseKept')
                : t('configurationBackup.passphraseHint', { min })
            }}
          </p>
        </SettingRow>
        <SettingRow
          v-if="draft.passphrase"
          :label="t('configurationBackup.passphraseRepeat')"
          for="plan-repeat"
        >
          <Input
            id="plan-repeat"
            v-model="draft.repeat"
            type="password"
            autocomplete="new-password"
          />
          <p v-if="planDiffers" class="text-xs text-destructive">
            {{ t('configurationBackup.passphrasesDiffer') }}
          </p>
        </SettingRow>
        <div class="grid gap-1 text-sm text-muted-foreground" data-testid="plan-state">
          <p v-if="plan.next_at">
            {{ t('configurationBackup.plan.next', { time: formatDateTime(plan.next_at * 1000) }) }}
          </p>
          <p v-if="plan.account">{{ t('configurationBackup.plan.by', { name: plan.account }) }}</p>
          <p v-if="last && last.succeeded">
            {{
              t('configurationBackup.plan.lastOk', {
                time: formatDateTime(last.at * 1000),
                file: last.file,
              })
            }}
          </p>
          <p v-else-if="last" class="text-destructive">
            {{
              t('configurationBackup.plan.lastFailed', {
                time: formatDateTime(last.at * 1000),
                error: last.error,
              })
            }}
          </p>
          <p v-if="planMessage" data-testid="plan-message">{{ planMessage }}</p>
        </div>
        <div class="flex flex-wrap justify-end gap-2">
          <Button
            type="button"
            variant="outline"
            :disabled="!plan.passphrase_set || !plan.target"
            @click="runNow"
          >
            {{ t('configurationBackup.plan.runNow') }}
          </Button>
          <Button type="submit" :disabled="save.isPending.value || planDiffers">
            {{ t('configurationBackup.plan.save') }}
          </Button>
        </div>
      </form>
    </PageCard>

    <PageCard :title="t('configurationBackup.restore.title')" :icon="ArchiveRestore">
      <div v-if="restored" class="grid gap-2 text-sm" data-testid="restored">
        <p class="font-medium">
          {{
            t('configurationBackup.restore.done', {
              name: restored.host_name,
              time: formatDateTime(restored.created_at * 1000),
              version: restored.version,
            })
          }}
        </p>
        <ul v-if="restored.warnings.length" class="list-disc pl-5 text-muted-foreground">
          <li v-for="warning in restored.warnings" :key="warning">{{ warning }}</li>
        </ul>
        <p v-if="restored.https_port">
          {{ t('configurationBackup.restore.port', { port: restored.https_port }) }}
        </p>
        <p v-if="restored.network.length" class="text-muted-foreground">
          {{ t('configurationBackup.restore.network', { files: restored.network.join(', ') }) }}
        </p>
        <p class="text-muted-foreground">{{ t('configurationBackup.restore.restarting') }}</p>
      </div>
      <form v-else class="grid gap-4" @submit.prevent="confirming = true">
        <p class="text-sm text-muted-foreground">{{ t('configurationBackup.restore.intro') }}</p>
        <Alert v-if="restoreError" variant="destructive">{{ restoreError }}</Alert>
        <SettingRow :label="t('configurationBackup.restore.file')" for="restore-file">
          <Input
            id="restore-file"
            type="file"
            accept=".age,application/octet-stream"
            @change="chosen"
          />
        </SettingRow>
        <SettingRow :label="t('configurationBackup.passphrase')" for="restore-passphrase">
          <Input
            id="restore-passphrase"
            v-model="restorePassphrase"
            type="password"
            autocomplete="off"
            required
          />
        </SettingRow>
        <div class="flex justify-end">
          <Button
            type="submit"
            variant="destructive"
            data-testid="restore"
            :disabled="restoring || !backupFile || !restorePassphrase"
          >
            {{ t('configurationBackup.restore.submit') }}
          </Button>
        </div>
      </form>
    </PageCard>

    <ConfirmDialog
      v-model:open="confirming"
      :title="t('configurationBackup.restore.confirmTitle')"
      :description="t('configurationBackup.restore.confirm')"
      :confirm-label="t('configurationBackup.restore.confirmLabel')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="restore"
    />
  </div>
</template>
