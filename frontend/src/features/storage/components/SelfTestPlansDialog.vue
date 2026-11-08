<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SelfTestFrequency, SelfTestKind, SelfTestPlan } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'

import { saveSelfTestPlans, useDisks, useRefreshStorage, useSelfTestPlans } from '../api'
import { formatHour, formatUtcOffset } from '../format'

const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const { data: plans, error } = useSelfTestPlans()
const { data: disks } = useDisks()
const refresh = useRefreshStorage()

const KINDS: SelfTestKind[] = ['short', 'long']
/** A long test takes hours: weekly at most. */
const FREQUENCIES: Record<SelfTestKind, SelfTestFrequency[]> = {
  short: ['daily', 'weekly', 'monthly'],
  long: ['weekly', 'monthly'],
}
const HOURS = Array.from({ length: 24 }, (_, hour) => hour)

interface Draft {
  on: boolean
  frequency: SelfTestFrequency
  hour: number
  /** Every disk that can, or only those in `disks`. */
  all: boolean
  /** Serial numbers. */
  disks: string[]
}

function defaults(): Record<SelfTestKind, Draft> {
  return {
    short: { on: false, frequency: 'weekly', hour: 2, all: true, disks: [] },
    long: { on: false, frequency: 'monthly', hour: 3, all: true, disks: [] },
  }
}

const drafts = ref<Record<SelfTestKind, Draft>>(defaults())
const failure = ref<string | null>(null)
const saving = ref(false)
/** Whether the form was filled from the server since the dialog opened. */
let filled = false

// Opening the dialog starts from what the server has, once it is loaded;
// a later refetch does not overwrite what is being edited.
watch(
  [open, () => plans.value],
  ([isOpen, loaded]) => {
    if (!isOpen) {
      filled = false
      return
    }
    if (filled || !loaded) return
    filled = true
    const next = defaults()
    for (const plan of plans.value?.plans ?? []) {
      next[plan.kind] = {
        on: true,
        frequency: plan.frequency,
        hour: plan.hour,
        all: !plan.disks?.length,
        disks: [...(plan.disks ?? [])],
      }
    }
    drafts.value = next
    failure.value = null
  },
  { immediate: true },
)

/** Disks with a serial number, and serials of plans whose disk is gone. */
const choices = computed(() => {
  const present = (disks.value ?? [])
    .filter((disk) => disk.serial)
    .map((disk) => ({
      serial: disk.serial ?? '',
      label: `${disk.name} · ${disk.model ?? t('storage.unknownModel')}`,
      detail: disk.serial ?? '',
    }))
  const known = new Set(present.map((choice) => choice.serial))
  const gone = KINDS.flatMap((kind) => drafts.value[kind].disks)
    .filter((serial, index, all) => !known.has(serial) && all.indexOf(serial) === index)
    .map((serial) => ({
      serial,
      label: serial,
      detail: t('storage.selfTestPlans.notConnected'),
    }))
  return [...present, ...gone]
})

/** A plan for chosen disks needs at least one. */
const incomplete = computed(() =>
  KINDS.some((kind) => {
    const draft = drafts.value[kind]
    return draft.on && !draft.all && draft.disks.length === 0
  }),
)

function lastRun(kind: SelfTestKind): string {
  const at = plans.value?.plans.find((plan) => plan.kind === kind)?.last_run_at
  return at
    ? t('storage.selfTestPlans.lastRun', { time: formatDateTime(at * 1000) })
    : t('storage.selfTestPlans.neverRun')
}

async function save(): Promise<void> {
  const body: SelfTestPlan[] = KINDS.filter((kind) => drafts.value[kind].on).map((kind) => {
    const draft = drafts.value[kind]
    return {
      kind,
      frequency: draft.frequency,
      hour: draft.hour,
      disks: draft.all ? [] : draft.disks,
    }
  })
  failure.value = null
  saving.value = true
  try {
    await saveSelfTestPlans(body)
    await refresh()
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}
</script>

<!-- When the disks test themselves: one plan per kind of test. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('storage.selfTestPlans.title')"
    :description="t('storage.selfTestPlans.description')"
    :close-label="t('desktop.close')"
  >
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>

    <form class="grid gap-4" @submit.prevent="save">
      <fieldset v-for="kind in KINDS" :key="kind" class="grid gap-2 rounded-lg border p-3">
        <label class="flex items-center gap-2 text-sm font-medium">
          <input v-model="drafts[kind].on" type="checkbox" class="size-4 accent-primary" />
          {{ t(`storage.selfTest.${kind}`) }}
        </label>
        <template v-if="drafts[kind].on">
          <div class="flex flex-wrap items-center gap-2 text-sm">
            <select
              v-model="drafts[kind].frequency"
              class="h-8 rounded-md border bg-background px-2 text-sm"
              :aria-label="t(`storage.selfTest.${kind}`)"
            >
              <option v-for="frequency in FREQUENCIES[kind]" :key="frequency" :value="frequency">
                {{ t(`storage.selfTestPlans.frequency.${frequency}`) }}
              </option>
            </select>
            <span class="text-muted-foreground">{{ t('storage.selfTestPlans.from') }}</span>
            <select
              v-model.number="drafts[kind].hour"
              class="h-8 rounded-md border bg-background px-2 text-sm tabular-nums"
              :aria-label="t('storage.selfTestPlans.from')"
            >
              <option v-for="hour in HOURS" :key="hour" :value="hour">
                {{ formatHour(hour) }}
              </option>
            </select>
          </div>
          <p v-if="kind === 'long'" class="text-xs text-muted-foreground">
            {{ t('storage.selfTestPlans.longHint') }}
          </p>
          <label class="flex items-center gap-2 text-sm">
            <input
              v-model="drafts[kind].all"
              type="radio"
              :name="`${kind}-disks`"
              :value="true"
              class="size-4 accent-primary"
            />
            {{ t('storage.selfTestPlans.allDisks') }}
          </label>
          <label class="flex items-center gap-2 text-sm">
            <input
              v-model="drafts[kind].all"
              type="radio"
              :name="`${kind}-disks`"
              :value="false"
              class="size-4 accent-primary"
            />
            {{ t('storage.selfTestPlans.chosenDisks') }}
          </label>
          <div v-if="!drafts[kind].all" class="grid gap-1 pl-6">
            <label
              v-for="choice in choices"
              :key="choice.serial"
              class="flex items-start gap-2 text-sm"
            >
              <input
                v-model="drafts[kind].disks"
                type="checkbox"
                :value="choice.serial"
                class="mt-0.5 size-4 shrink-0 accent-primary"
              />
              <span class="grid min-w-0">
                <span class="break-words">{{ choice.label }}</span>
                <span class="font-mono text-xs break-all text-muted-foreground">{{
                  choice.detail
                }}</span>
              </span>
            </label>
          </div>
          <p class="text-xs text-muted-foreground">{{ lastRun(kind) }}</p>
        </template>
      </fieldset>

      <p v-if="plans" class="text-xs text-muted-foreground">
        {{
          t('storage.selfTestPlans.timeZone', {
            offset: formatUtcOffset(plans.utc_offset_seconds),
          })
        }}
      </p>
      <div class="flex justify-end">
        <Button type="submit" :disabled="saving || incomplete">
          {{ t('storage.selfTestPlans.save') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
