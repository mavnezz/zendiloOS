<script setup lang="ts">
import { Camera, Trash2 } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Share, ShareSnapshotSchedule, SnapshotFrequency } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useShareSnapshots, useSnapshotActions } from '../api'

const props = defineProps<{ share: Share | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const id = computed(() => props.share?.id ?? null)
const { data, error } = useShareSnapshots(id)
const actions = useSnapshotActions()

const FREQUENCIES: SnapshotFrequency[] = ['hourly', 'daily', 'weekly', 'monthly']
/** What a schedule keeps when it is switched on. */
const DEFAULT_KEEP: Record<SnapshotFrequency, number> = {
  hourly: 24,
  daily: 7,
  weekly: 4,
  monthly: 12,
}

interface Plan {
  on: boolean
  /** As typed; a number from 1 to 100. */
  keep: string
}
const plans = ref<Record<SnapshotFrequency, Plan>>(emptyPlans())
const note = ref('')
const confirming = ref<string | null>(null)
const failure = ref<string | null>(null)
const saved = ref(false)

function emptyPlans(): Record<SnapshotFrequency, Plan> {
  return {
    hourly: { on: false, keep: String(DEFAULT_KEEP.hourly) },
    daily: { on: false, keep: String(DEFAULT_KEEP.daily) },
    weekly: { on: false, keep: String(DEFAULT_KEEP.weekly) },
    monthly: { on: false, keep: String(DEFAULT_KEEP.monthly) },
  }
}

// The form follows the server's schedules whenever they are loaded anew.
watch(
  () => data.value?.schedules,
  (schedules) => {
    const next = emptyPlans()
    for (const schedule of schedules ?? []) {
      next[schedule.frequency] = { on: true, keep: String(schedule.keep) }
    }
    plans.value = next
  },
  { immediate: true },
)

watch(open, (isOpen) => {
  if (!isOpen) return
  note.value = ''
  confirming.value = null
  failure.value = null
  saved.value = false
})

async function run(action: () => Promise<unknown>): Promise<void> {
  failure.value = null
  try {
    await action()
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

function saveSchedules(): Promise<void> {
  const share = props.share
  if (!share) return Promise.resolve()
  const body: ShareSnapshotSchedule[] = FREQUENCIES.filter((f) => plans.value[f].on).map((f) => ({
    frequency: f,
    keep: Math.round(Number(plans.value[f].keep)),
  }))
  return run(async () => {
    await actions.schedule(share.id, body)
    saved.value = true
  })
}

function take(): Promise<void> {
  const share = props.share
  if (!share) return Promise.resolve()
  return run(async () => {
    await actions.take(share.id, note.value)
    note.value = ''
  })
}

function remove(label: string): Promise<void> {
  const share = props.share
  if (!share) return Promise.resolve()
  return run(async () => {
    await actions.remove(share.id, label)
    confirming.value = null
  })
}
</script>

<!-- A share's snapshots: schedules, one taken by hand, the list. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('shares.snapshots.title', { name: share?.name ?? '' })"
    :description="t('shares.snapshots.description')"
    :close-label="t('desktop.close')"
  >
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>

    <form class="grid gap-2" @submit.prevent="saveSchedules">
      <h3 class="text-sm font-medium">{{ t('shares.snapshots.schedules') }}</h3>
      <div
        v-for="frequency in FREQUENCIES"
        :key="frequency"
        class="flex items-center gap-3 text-sm"
      >
        <label class="flex flex-1 items-center gap-2">
          <input v-model="plans[frequency].on" type="checkbox" class="size-4" />
          {{ t(`shares.snapshots.frequency.${frequency}`) }}
        </label>
        <Label :for="`keep-${frequency}`" class="text-xs text-muted-foreground">
          {{ t('shares.snapshots.keep') }}
        </Label>
        <Input
          :id="`keep-${frequency}`"
          v-model="plans[frequency].keep"
          type="number"
          min="1"
          max="100"
          class="h-8 w-20"
          :disabled="!plans[frequency].on"
        />
      </div>
      <div class="flex items-center justify-end gap-2">
        <span v-if="saved" class="text-xs text-muted-foreground">{{
          t('shares.snapshots.saved')
        }}</span>
        <Button type="submit" size="sm" variant="outline">{{ t('shares.snapshots.save') }}</Button>
      </div>
    </form>

    <form class="grid gap-2" @submit.prevent="take">
      <Label for="snapshot-note">{{ t('shares.snapshots.note') }}</Label>
      <div class="flex gap-2">
        <Input id="snapshot-note" v-model="note" maxlength="200" autocomplete="off" />
        <Button type="submit" size="sm">
          <Camera />
          {{ t('shares.snapshots.take') }}
        </Button>
      </div>
    </form>

    <div class="grid gap-1">
      <h3 class="text-sm font-medium">{{ t('shares.snapshots.list') }}</h3>
      <p v-if="data && !data.snapshots.length" class="text-sm text-muted-foreground">
        {{ t('shares.snapshots.none') }}
      </p>
      <ul class="divide-y rounded-md border">
        <li
          v-for="snapshot in data?.snapshots ?? []"
          :key="snapshot.label"
          class="flex items-center gap-2 px-3 py-2 text-sm"
        >
          <div class="min-w-0 flex-1">
            <p class="font-medium tabular-nums">{{ formatDateTime(snapshot.taken_at * 1000) }}</p>
            <p class="truncate text-xs text-muted-foreground">
              <template v-if="snapshot.note">{{ snapshot.note }} · </template>
              {{ formatBytes(snapshot.used_bytes) }}
            </p>
          </div>
          <Badge v-if="!snapshot.schedules.length" variant="outline">
            {{ t('shares.snapshots.byHand') }}
          </Badge>
          <Badge v-for="frequency in snapshot.schedules" :key="frequency" variant="secondary">
            {{ t(`shares.snapshots.frequency.${frequency}`) }}
          </Badge>
          <template v-if="confirming === snapshot.label">
            <Button size="sm" variant="destructive" @click="remove(snapshot.label)">
              {{ t('shares.snapshots.confirmDelete') }}
            </Button>
            <Button size="sm" variant="ghost" @click="confirming = null">
              {{ t('desktop.cancel') }}
            </Button>
          </template>
          <Button
            v-else
            variant="ghost"
            size="icon"
            :title="t('users.delete')"
            :aria-label="t('users.delete')"
            @click="confirming = snapshot.label"
          >
            <Trash2 />
          </Button>
        </li>
      </ul>
    </div>
  </ModalDialog>
</template>
