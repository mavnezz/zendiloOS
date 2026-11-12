<script setup lang="ts">
import { Camera, History, Info, Trash2 } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmSnapshot } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { ActionMenu, type Action } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useSnapshotActions, useVmSnapshots } from '../api'
import {
  FREQUENCIES,
  KEEP_MAX,
  consistencyVariant,
  newerThan,
  scheduleDraft,
  scheduleList,
  snapshotNameValid,
  type ScheduleDraft,
} from '../lib/backups'
import { isOn } from '../lib/vms'

/** A VM's snapshots: taken, rolled back while it is off, deleted, and on schedules. */
const props = defineProps<{ vm: VirtualMachine }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useSnapshotActions()
const { data, error, isPending } = useVmSnapshots(() => (open.value ? props.vm.id : null))

const name = ref('')
const taking = ref(false)
const failure = ref<string | null>(null)
const rolling = ref<VmSnapshot | null>(null)
const deleting = ref<VmSnapshot | null>(null)
const draft = ref<ScheduleDraft>(scheduleDraft([]))
const savedSchedules = ref(false)

watch(open, (isOpen) => {
  if (!isOpen) return
  name.value = ''
  failure.value = null
  savedSchedules.value = false
})
watch(
  () => data.value?.schedules,
  (schedules) => {
    if (schedules) draft.value = scheduleDraft(schedules)
  },
  { immediate: true },
)

const snapshots = computed(() => data.value?.snapshots ?? [])
const off = computed(() => props.vm.state === 'shut_off')
const later = computed(() => (rolling.value ? newerThan(snapshots.value, rolling.value.label) : []))
const schedules = computed(() => scheduleList(draft.value))
const when = (snapshot: VmSnapshot | null) =>
  snapshot ? formatDateTime(snapshot.taken_at * 1000) : ''

async function run(work: () => Promise<unknown>): Promise<void> {
  failure.value = null
  try {
    await work()
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

async function take(): Promise<void> {
  if (!snapshotNameValid(name.value)) return
  taking.value = true
  await run(async () => {
    await actions.take(props.vm.id, name.value.trim())
    name.value = ''
  })
  taking.value = false
}

function rollback(): void {
  const snapshot = rolling.value
  const newer = later.value.length > 0
  if (snapshot) void run(() => actions.rollback(props.vm.id, snapshot.label, newer))
}

function remove(): void {
  const snapshot = deleting.value
  if (snapshot) void run(() => actions.remove(props.vm.id, snapshot.label))
}

async function saveSchedules(): Promise<void> {
  const list = schedules.value
  if (!list) return
  savedSchedules.value = false
  await run(async () => {
    await actions.setSchedules(props.vm.id, list)
    savedSchedules.value = true
  })
}

function toggle(frequency: keyof ScheduleDraft, on: boolean): void {
  const defaults = { hourly: 24, daily: 7, weekly: 4, monthly: 6 }
  draft.value = { ...draft.value, [frequency]: on ? defaults[frequency] : null }
}

function setKeep(frequency: keyof ScheduleDraft, value: string | number): void {
  draft.value = { ...draft.value, [frequency]: Number(value) }
}

function menu(): Action[] {
  return [
    {
      id: 'rollback',
      label: off.value ? t('vms.snapshots.rollback') : t('vms.snapshots.rollbackOff'),
      icon: History,
      disabled: !off.value,
    },
    { id: 'delete', label: t('vms.snapshots.delete'), icon: Trash2, destructive: true },
  ]
}

function select(snapshot: VmSnapshot, id: string): void {
  if (id === 'rollback') rolling.value = snapshot
  else deleting.value = snapshot
}

const hint = 'text-xs text-muted-foreground'
</script>

<!-- A VM's snapshots: every disk at one moment, by hand and on schedules. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('vms.snapshots.title', { name: vm.name })"
    :description="t('vms.snapshots.description')"
    :close-label="t('desktop.close')"
    wide
  >
    <div class="grid gap-5">
      <Alert v-if="isOn(vm.state)" data-testid="vm-snapshot-running">
        <Info />
        {{ t('vms.snapshots.running') }}
      </Alert>
      <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
      <Alert v-if="failure" variant="destructive" data-testid="vm-snapshot-failure">{{
        failure
      }}</Alert>

      <form id="vm-snapshot-take" class="flex flex-wrap items-end gap-2" @submit.prevent="take">
        <div class="grid min-w-48 flex-1 gap-2">
          <Label for="vm-snapshot-name">{{ t('vms.snapshots.name') }}</Label>
          <Input
            id="vm-snapshot-name"
            v-model="name"
            maxlength="64"
            autocomplete="off"
            :placeholder="t('vms.snapshots.namePlaceholder')"
            :aria-invalid="!snapshotNameValid(name)"
          />
        </div>
        <Button type="submit" :disabled="taking || !snapshotNameValid(name)">
          <Camera />
          {{ taking ? t('vms.snapshots.taking') : t('vms.snapshots.take') }}
        </Button>
      </form>

      <div v-if="isPending" class="h-24 animate-pulse rounded-xl bg-muted" />
      <p
        v-else-if="!snapshots.length"
        class="text-sm text-muted-foreground"
        data-testid="vm-snapshots-empty"
      >
        {{ t('vms.snapshots.empty') }}
      </p>
      <ul v-else class="grid gap-2">
        <li
          v-for="snapshot in snapshots"
          :key="snapshot.label"
          :data-snapshot="snapshot.label"
          class="flex flex-wrap items-center gap-3 rounded-xl border p-3"
        >
          <div class="grid min-w-0 flex-1 gap-0.5">
            <span class="truncate font-medium">{{
              snapshot.name ?? t('vms.snapshots.unnamed')
            }}</span>
            <span :class="hint">
              {{ when(snapshot) }} ·
              {{
                t(
                  'vms.snapshots.disks',
                  { disks: snapshot.disks.map((slot) => slot + 1).join(', ') },
                  snapshot.disks.length,
                )
              }}
              ·
              {{
                t('vms.snapshots.size', {
                  used: formatBytes(snapshot.used_bytes),
                  referenced: formatBytes(snapshot.referenced_bytes),
                })
              }}
            </span>
          </div>
          <span class="flex flex-wrap gap-1">
            <Badge
              :variant="consistencyVariant(snapshot.consistency)"
              :title="t(`vms.snapshots.consistencyHint.${snapshot.consistency}`)"
              >{{ t(`vms.snapshots.consistency.${snapshot.consistency}`) }}</Badge
            >
            <Badge v-for="frequency in snapshot.schedules" :key="frequency" variant="outline">{{
              t(`vms.snapshots.frequencies.${frequency}`)
            }}</Badge>
            <Badge v-if="!snapshot.schedules.length" variant="outline">{{
              t('vms.snapshots.byHand')
            }}</Badge>
          </span>
          <ActionMenu
            :actions="menu()"
            :label="t('vms.snapshots.more', { when: when(snapshot) })"
            @select="(id: string) => select(snapshot, id)"
          />
        </li>
      </ul>
      <p v-if="snapshots.some((s) => s.consistency === 'crash')" :class="hint">
        {{ t('vms.snapshots.consistencyHint.crash') }}
      </p>

      <form
        id="vm-snapshot-schedules"
        class="grid gap-3 border-t pt-4"
        @submit.prevent="saveSchedules"
      >
        <div class="grid gap-0.5">
          <h3 class="text-sm font-medium">{{ t('vms.snapshots.schedules') }}</h3>
          <p :class="hint">{{ t('vms.snapshots.schedulesHint') }}</p>
        </div>
        <div class="grid gap-2 sm:grid-cols-2">
          <div
            v-for="frequency in FREQUENCIES"
            :key="frequency"
            class="flex items-center gap-3 rounded-lg border p-2 text-sm"
            :data-frequency="frequency"
          >
            <label class="flex flex-1 items-center gap-2">
              <input
                type="checkbox"
                class="size-4 accent-primary"
                :checked="draft[frequency] !== null"
                @change="toggle(frequency, ($event.target as HTMLInputElement).checked)"
              />
              {{ t(`vms.snapshots.frequencies.${frequency}`) }}
            </label>
            <Input
              v-if="draft[frequency] !== null"
              :id="`vm-keep-${frequency}`"
              :model-value="draft[frequency] ?? ''"
              type="number"
              min="1"
              :max="KEEP_MAX"
              class="w-20 tabular-nums"
              :aria-label="
                t('vms.snapshots.keepFor', {
                  frequency: t(`vms.snapshots.frequencies.${frequency}`),
                })
              "
              @update:model-value="(value) => setKeep(frequency, value)"
            />
          </div>
        </div>
        <p v-if="!schedules" class="text-xs text-destructive">
          {{ t('vms.snapshots.keepInvalid') }}
        </p>
        <p v-if="savedSchedules" :class="hint" data-testid="vm-schedules-saved">
          {{ t('vms.snapshots.saved') }}
        </p>
        <div>
          <Button type="submit" variant="outline" :disabled="!schedules">
            {{ t('vms.snapshots.saveSchedules') }}
          </Button>
        </div>
      </form>
    </div>

    <ConfirmDialog
      :open="Boolean(rolling)"
      :title="t('vms.snapshots.rollbackTitle', { name: vm.name, when: when(rolling) })"
      :description="
        [
          t('vms.snapshots.rollbackText'),
          later.length
            ? t('vms.snapshots.rollbackNewer', { labels: later.join(', ') }, later.length)
            : '',
        ]
          .filter(Boolean)
          .join(' ')
      "
      :confirm-label="t('vms.snapshots.rollback')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(isOpen: boolean) => !isOpen && (rolling = null)"
      @confirm="rollback"
    />
    <ConfirmDialog
      :open="Boolean(deleting)"
      :title="t('vms.snapshots.deleteTitle', { when: when(deleting) })"
      :description="t('vms.snapshots.deleteText')"
      :confirm-label="t('vms.snapshots.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(isOpen: boolean) => !isOpen && (deleting = null)"
      @confirm="remove"
    />
  </ModalDialog>
</template>
