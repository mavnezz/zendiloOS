<script setup lang="ts">
import { Database, RefreshCw, Search } from 'lucide-vue-next'
import { computed, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerDataLocation, ImageUpdateSettings } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { PageFooter } from '@/shared/ui/page'

import {
  useApplyUpdates,
  useCheckUpdates,
  useDataLocation,
  useMoveData,
  useSetUpdateSettings,
  useUpdateSettings,
  useUpdates,
} from '../api'

const { t } = useI18n()
const { data: settings } = useUpdateSettings()
const save = useSetUpdateSettings()
const { data: report } = useUpdates()
const check = useCheckUpdates()
const apply = useApplyUpdates()
const { data: location } = useDataLocation()
const move = useMoveData()

interface Draft {
  check: boolean
  scheduled: boolean
  days: boolean[]
  time: string
}
const draft = ref<Draft>()
const base = shallowRef<Draft>()

function toDraft(value: ImageUpdateSettings): Draft {
  return {
    check: value.check,
    scheduled: value.schedule != null,
    days: Array.from({ length: 7 }, (_, day) => value.schedule?.days.includes(day) ?? day === 0),
    time: value.schedule?.time ?? '05:40',
  }
}
function reset(): void {
  if (!settings.value) return
  base.value = toDraft(settings.value)
  draft.value = toDraft(settings.value)
  save.reset()
}
const dirty = computed(() => JSON.stringify(draft.value) !== JSON.stringify(base.value))
watch(settings, () => !dirty.value && reset(), { immediate: true })

const valid = computed(() => {
  const value = draft.value
  return !value?.scheduled || (value.days.includes(true) && /^\d{2}:\d{2}$/.test(value.time))
})

function store(): void {
  const value = draft.value
  if (!value || !valid.value) return
  save.mutate(
    {
      check: value.check || value.scheduled,
      schedule: value.scheduled
        ? { days: value.days.flatMap((on, day) => (on ? [day] : [])), time: value.time }
        : null,
    },
    { onSuccess: reset },
  )
}

const available = computed(() =>
  (report.value?.images ?? []).filter((i) => i.state === 'available'),
)
const unknown = computed(() => (report.value?.images ?? []).filter((i) => i.state === 'unknown'))

const moving = ref(false)
const target = ref('')
const confirming = ref(false)
watch(moving, (open) => open && (target.value = ''))
const chosen = computed(() => location.value?.targets.find((p) => p.pool === target.value))
const fits = (place: DockerDataLocation['targets'][number]) =>
  place.available_bytes == null || place.available_bytes >= (location.value?.data_bytes ?? 0)

function start(): void {
  move.mutate(target.value, {
    onSuccess: () => {
      moving.value = false
    },
  })
}

const weekdays = computed(() =>
  [0, 1, 2, 3, 4, 5, 6].map((day) => t(`docker.management.days.${day}`)),
)
const card = 'grid gap-4 rounded-xl border bg-card p-5'
</script>

<!--
  How images stay current, checked and taken on a schedule; and where
  Docker keeps its data.
-->
<template>
  <div class="flex min-h-full flex-col gap-4">
    <section v-if="draft" :class="card">
      <h3 class="font-semibold">{{ t('docker.management.updates') }}</h3>
      <label class="flex items-start gap-2.5 text-sm">
        <input v-model="draft.check" type="checkbox" class="mt-0.5 size-4 accent-primary" />
        <span>
          {{ t('docker.management.check') }}
          <span class="block text-xs text-muted-foreground">{{
            t('docker.management.checkHint')
          }}</span>
        </span>
      </label>
      <div class="grid gap-3 pl-6">
        <label class="flex items-start gap-2.5 text-sm">
          <input
            v-model="draft.scheduled"
            type="checkbox"
            class="mt-0.5 size-4 accent-primary"
            :disabled="!draft.check"
          />
          <span>
            {{ t('docker.management.schedule') }}
            <span class="block text-xs text-muted-foreground">{{
              t('docker.management.scheduleHint')
            }}</span>
          </span>
        </label>
        <div v-if="draft.scheduled" class="flex flex-wrap items-center gap-2 pl-6">
          <button
            v-for="(day, index) in weekdays"
            :key="index"
            type="button"
            class="rounded-md border px-2.5 py-1 text-sm transition-colors"
            :class="
              draft.days[index]
                ? 'border-primary bg-primary/10 text-primary'
                : 'text-muted-foreground hover:bg-accent'
            "
            :aria-pressed="draft.days[index]"
            @click="draft.days[index] = !draft.days[index]"
          >
            {{ day }}
          </button>
          <Input
            v-model="draft.time"
            type="time"
            class="w-32"
            :aria-label="t('docker.management.time')"
          />
        </div>
      </div>

      <div class="grid gap-2 border-t pt-4">
        <div class="flex flex-wrap items-center gap-2">
          <span class="text-sm text-muted-foreground">
            {{
              report
                ? t('docker.management.checkedAt', { at: formatDateTime(report.checked_at * 1000) })
                : t('docker.management.notChecked')
            }}
          </span>
          <Button
            variant="secondary"
            size="sm"
            class="ml-auto"
            :disabled="check.isPending.value"
            @click="check.mutate()"
          >
            <Search />
            {{ t('docker.management.checkNow') }}
          </Button>
          <Button
            size="sm"
            :disabled="!available.length || apply.isPending.value"
            @click="apply.mutate()"
          >
            <RefreshCw />
            {{ t('docker.management.updateNow') }}
          </Button>
        </div>
        <Alert v-if="check.error.value || apply.error.value" variant="destructive">{{
          problemMessage(check.error.value ?? apply.error.value)
        }}</Alert>
        <ul v-if="available.length" class="grid gap-1.5 text-sm">
          <li
            v-for="image in available"
            :key="image.reference"
            class="flex flex-wrap items-center gap-2"
          >
            <Badge variant="info">{{ t('docker.management.newer') }}</Badge>
            <span class="font-mono text-dense">{{ image.reference }}</span>
            <span class="text-muted-foreground">{{ image.containers.join(', ') }}</span>
          </li>
        </ul>
        <p v-else-if="report" class="text-sm text-muted-foreground">
          {{ t('docker.management.current') }}
        </p>
        <details v-if="unknown.length" class="text-xs text-muted-foreground">
          <summary class="cursor-pointer">
            {{ t('docker.management.unknown', { n: unknown.length }) }}
          </summary>
          <ul class="mt-1 grid gap-0.5">
            <li v-for="image in unknown" :key="image.reference">
              <span class="font-mono">{{ image.reference }}</span
              >: {{ image.reason }}
            </li>
          </ul>
        </details>
      </div>
    </section>

    <section :class="card">
      <h3 class="font-semibold">{{ t('docker.management.migration') }}</h3>
      <p class="text-sm text-muted-foreground">{{ t('docker.management.migrationHint') }}</p>
      <p class="text-sm">
        <template v-if="location?.current">
          {{
            t('docker.management.currently', {
              pool: location.current.pool,
              size: formatBytes(location.data_bytes),
            })
          }}
        </template>
        <template v-else-if="location">{{ t('docker.management.notOnPool') }}</template>
      </p>
      <div>
        <Button
          variant="secondary"
          :disabled="!location?.current || !location.targets.length"
          @click="moving = true"
        >
          <Database />
          {{ t('docker.management.migrate') }}
        </Button>
      </div>
    </section>

    <div class="grow" aria-hidden="true" />
    <PageFooter
      :dirty="dirty"
      :saving="save.isPending.value"
      :valid="valid"
      :error="save.error.value ? problemMessage(save.error.value) : null"
      @reset="reset()"
      @apply="store()"
    />

    <ModalDialog
      v-model:open="moving"
      :title="t('docker.management.chooseTitle')"
      :close-label="t('desktop.close')"
    >
      <div class="grid gap-3">
        <p class="text-sm">
          {{ t('docker.management.chooseText', { pool: location?.current?.pool }) }}
        </p>
        <p class="text-xs text-muted-foreground">{{ t('docker.management.stopsAll') }}</p>
        <button
          v-for="place in location?.targets"
          :key="place.pool"
          type="button"
          class="flex items-center gap-3 rounded-xl border p-3 text-left transition-colors disabled:opacity-50"
          :class="target === place.pool ? 'border-primary bg-primary/5' : 'hover:bg-accent'"
          :disabled="!fits(place)"
          @click="target = place.pool"
        >
          <Database class="size-6 text-muted-foreground" />
          <span class="grid flex-1">
            <span class="font-medium">{{ place.pool }}</span>
            <span class="text-xs text-muted-foreground">{{
              t('docker.management.volume', { n: place.volume })
            }}</span>
          </span>
          <span class="text-xs text-muted-foreground">{{
            place.available_bytes != null
              ? t('docker.management.free', { size: formatBytes(place.available_bytes) })
              : ''
          }}</span>
        </button>
        <Alert v-if="move.error.value" variant="destructive">{{
          problemMessage(move.error.value)
        }}</Alert>
        <div class="flex justify-end gap-2">
          <Button variant="ghost" @click="moving = false">{{ t('desktop.cancel') }}</Button>
          <Button :disabled="!chosen || move.isPending.value" @click="confirming = true">{{
            t('docker.management.start')
          }}</Button>
        </div>
      </div>
    </ModalDialog>
    <ConfirmDialog
      v-model:open="confirming"
      :title="t('docker.management.confirmTitle', { pool: target })"
      :description="t('docker.management.confirmText')"
      :confirm-label="t('docker.management.start')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="start"
    />
  </div>
</template>
