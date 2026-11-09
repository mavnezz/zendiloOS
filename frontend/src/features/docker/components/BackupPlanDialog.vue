<script setup lang="ts">
import { FolderOpen } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerVolume, PlanFrequency } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { FolderPicker, useShareList } from '@/shared/ui/folder-picker'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import {
  deleteBackupPlan,
  saveBackupPlan,
  useBackupReaders,
  useRefreshDocker,
  useVolumes,
} from '../api'
import {
  defaultPlan,
  hourText,
  pickerPath,
  planTarget,
  quietHour,
  readersText,
} from '../lib/volumes'

const props = defineProps<{ volume: DockerVolume | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const { data: shares } = useShareList()
const { data: volumes } = useVolumes()
const refresh = useRefreshDocker()

/** A new plan's hour: the night's hour the other plans take least. */
function newPlan(volume: string) {
  const others = (volumes.value ?? []).filter((other) => other.name !== volume)
  return defaultPlan(
    quietHour(
      volume,
      others.flatMap((other) => (other.plan ? [other.plan] : [])),
    ),
  )
}

const FREQUENCIES: PlanFrequency[] = ['daily', 'weekly', 'monthly']
const HOURS = Array.from({ length: 24 }, (_, hour) => hour)
const DAYS = Array.from({ length: 31 }, (_, day) => day + 1)

const draft = ref(defaultPlan())
const folder = ref('')
const browsing = ref(false)
const ending = ref(false)
const saving = ref(false)
const failure = ref<string | null>(null)

// Each opening starts from the volume's plan, or from the defaults.
watch(
  [open, () => shares.value],
  ([isOpen]) => {
    if (!isOpen || !props.volume) return
    const plan = props.volume.plan
    failure.value = null
    draft.value = plan
      ? {
          frequency: plan.frequency,
          weekday: plan.weekday ?? 6,
          day: plan.day ?? 1,
          hour: plan.hour,
          keep: plan.keep,
          stop: plan.stop_containers,
        }
      : newPlan(props.volume.name)
    folder.value = plan ? pickerPath(plan, shares.value ?? []) : ''
  },
  { immediate: true },
)

const writable = computed(() => (shares.value ?? []).filter((share) => share.writable))
const target = computed(() => planTarget(folder.value, writable.value))
const { data: readers } = useBackupReaders(computed(() => target.value?.share_id ?? null))
const readersLine = computed(() => (readers.value ? readersText(readers.value, t) : ''))
const keepValid = computed(
  () => Number.isInteger(draft.value.keep) && draft.value.keep >= 1 && draft.value.keep <= 365,
)

async function save(): Promise<void> {
  const volume = props.volume
  if (!volume || !target.value || !keepValid.value) return
  saving.value = true
  failure.value = null
  try {
    await saveBackupPlan(volume.name, {
      ...target.value,
      frequency: draft.value.frequency,
      weekday: draft.value.frequency === 'weekly' ? draft.value.weekday : null,
      day: draft.value.frequency === 'monthly' ? draft.value.day : null,
      hour: draft.value.hour,
      keep: draft.value.keep,
      stop_containers: draft.value.stop,
    })
    await refresh()
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}

async function end(): Promise<void> {
  const volume = props.volume
  if (!volume) return
  failure.value = null
  try {
    await deleteBackupPlan(volume.name)
    await refresh()
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

const label = 'text-sm font-medium'
const hint = 'text-xs text-muted-foreground'
const select = 'h-9 rounded-md border bg-background px-2 text-sm'
</script>

<!-- How one volume is backed up: where to, when, how many kept. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('docker.volumes.plan.title', { name: volume?.name ?? '' })"
    :description="t('docker.volumes.plan.hint')"
    :close-label="t('desktop.close')"
  >
    <form id="backup-plan" class="grid gap-4" @submit.prevent="save">
      <div class="grid gap-2">
        <Label for="backup-folder" :class="label">{{ t('docker.volumes.plan.folder') }}</Label>
        <div class="flex min-w-0 gap-2">
          <Input
            id="backup-folder"
            v-model="folder"
            class="min-w-0 flex-1 font-mono"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="folder.trim() !== '' && !target"
          />
          <Button type="button" variant="outline" class="shrink-0" @click="browsing = true">
            <FolderOpen />
            {{ t('docker.volumes.plan.browse') }}
          </Button>
        </div>
        <p :class="folder.trim() !== '' && !target ? 'text-xs text-destructive' : hint">
          {{ t('docker.volumes.plan.noFolder') }}
        </p>
        <div
          v-if="target"
          role="note"
          class="grid gap-1 rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs"
        >
          <p>{{ t('docker.volumes.plan.readable') }}</p>
          <p v-if="readers" class="text-muted-foreground">
            {{
              readersLine
                ? t('docker.volumes.plan.readers', { names: readersLine })
                : t('docker.volumes.plan.noReaders')
            }}
            <template v-if="readers.nfs.length">
              {{ t('docker.volumes.plan.nfs', { networks: readers.nfs.join(', ') }) }}</template
            >
          </p>
        </div>
      </div>

      <div class="flex flex-wrap items-end gap-3">
        <div class="grid gap-2">
          <Label for="backup-frequency" :class="label">{{
            t('docker.volumes.plan.frequency')
          }}</Label>
          <select id="backup-frequency" v-model="draft.frequency" :class="select">
            <option v-for="frequency in FREQUENCIES" :key="frequency" :value="frequency">
              {{ t(`docker.volumes.plan.frequencies.${frequency}`) }}
            </option>
          </select>
        </div>
        <div v-if="draft.frequency === 'weekly'" class="grid gap-2">
          <Label for="backup-weekday" :class="label">{{ t('docker.volumes.plan.weekday') }}</Label>
          <select id="backup-weekday" v-model.number="draft.weekday" :class="select">
            <option v-for="day in 7" :key="day" :value="day - 1">
              {{ t(`docker.volumes.weekdays.${day - 1}`) }}
            </option>
          </select>
        </div>
        <div v-if="draft.frequency === 'monthly'" class="grid gap-2">
          <Label for="backup-day" :class="label">{{ t('docker.volumes.plan.day') }}</Label>
          <select id="backup-day" v-model.number="draft.day" :class="select">
            <option v-for="day in DAYS" :key="day" :value="day">{{ day }}</option>
          </select>
        </div>
        <div class="grid gap-2">
          <Label for="backup-hour" :class="label">{{ t('docker.volumes.plan.hour') }}</Label>
          <select id="backup-hour" v-model.number="draft.hour" :class="[select, 'tabular-nums']">
            <option v-for="hour in HOURS" :key="hour" :value="hour">{{ hourText(hour) }}</option>
          </select>
        </div>
      </div>
      <p :class="hint">
        {{ t('docker.volumes.plan.hourHint') }}
        <template v-if="draft.frequency === 'monthly'">
          {{ t('docker.volumes.plan.dayHint') }}</template
        >
      </p>

      <div class="grid gap-2">
        <Label for="backup-keep" :class="label">{{ t('docker.volumes.plan.keep') }}</Label>
        <Input
          id="backup-keep"
          v-model.number="draft.keep"
          type="number"
          min="1"
          max="365"
          class="w-28 tabular-nums"
          :aria-invalid="!keepValid"
        />
        <p :class="hint">{{ t('docker.volumes.plan.keepHint') }}</p>
      </div>

      <label class="flex items-start gap-2 text-sm">
        <input v-model="draft.stop" type="checkbox" class="mt-0.5 size-4 accent-primary" />
        <span class="grid gap-0.5">
          <span class="font-medium">{{ t('docker.volumes.plan.stop') }}</span>
          <span :class="hint">{{ t('docker.volumes.plan.stopHint') }}</span>
        </span>
      </label>

      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </form>

    <template #footer>
      <Button
        v-if="volume?.plan"
        type="button"
        variant="ghost"
        class="mr-auto text-destructive"
        @click="ending = true"
        >{{ t('docker.volumes.plan.end') }}</Button
      >
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button type="submit" form="backup-plan" :disabled="saving || !target || !keepValid">{{
        t('docker.volumes.plan.save')
      }}</Button>
    </template>
    <FolderPicker v-model:open="browsing" :value="folder" writable @pick="folder = $event" />
    <ConfirmDialog
      v-model:open="ending"
      :title="t('docker.volumes.plan.endTitle', { name: volume?.name ?? '' })"
      :description="t('docker.volumes.plan.endText')"
      :confirm-label="t('docker.volumes.plan.end')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="end"
    />
  </ModalDialog>
</template>
