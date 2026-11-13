<script setup lang="ts">
import { CircleCheck, CircleDashed, Monitor, Plus, X } from 'lucide-vue-next'
import { computed, onScopeDispose, reactive, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { TimeState } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { fromWallTime, toWallTime } from '@/shared/lib/time-zone'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { PageDescription, PageFooter } from '@/shared/ui/page'
import { RadioGroup, RadioGroupItem } from '@/shared/ui/radio-group'

import { useSetTime, useTime } from '../api'

/** As the server allows. */
const MAX_SERVERS = 4

defineProps<{ canManage: boolean }>()

const { t, locale } = useI18n()
const { data, isPending } = useTime()
const setTime = useSetTime()

/**
 * Where a clock set by hand gets its time: not at all (the clock stays),
 * typed in, or this computer's clock at the moment of applying.
 */
type ClockSource = 'none' | 'typed' | 'browser'

interface Draft {
  zone: string
  mode: 'servers' | 'hand'
  servers: string[]
  /** Typed into the date-time field, as the NAS's zone shows it. */
  clock: string
  source: ClockSource
}

const draft = reactive<Draft>({ zone: '', mode: 'servers', servers: [], clock: '', source: 'none' })

/** The settings the draft started from. */
const base = shallowRef<TimeState>()

function reset(state: TimeState | undefined = data.value): void {
  if (!state) return
  base.value = state
  draft.zone = state.zone
  draft.mode = state.network_time ? 'servers' : 'hand'
  draft.servers = [...state.servers]
  draft.clock = ''
  draft.source = 'none'
  setTime.reset()
}

const servers = computed(() => draft.servers.map((server) => server.trim()).filter(Boolean))
const dirty = computed(() => {
  const state = base.value
  if (!state) return false
  return (
    draft.zone !== state.zone ||
    (draft.mode === 'servers') !== state.network_time ||
    servers.value.join(' ') !== state.servers.join(' ') ||
    (draft.mode === 'hand' && draft.source !== 'none')
  )
})

watch(
  data,
  // Newer settings replace the draft unless someone is editing it.
  (state) => {
    if (!base.value || !dirty.value) reset(state)
  },
  { immediate: true },
)

// The NAS's time, running on from when it was read: its clock, not the
// browser's, which may be off or in another zone.
const tick = ref(Date.now())
const timer = window.setInterval(() => (tick.value = Date.now()), 1000)
onScopeDispose(() => window.clearInterval(timer))
// Both from one reading, so the time shown never runs backwards.
const readAt = ref(tick.value)
watch(data, () => (readAt.value = tick.value = Date.now()))
const nasNow = computed(() =>
  data.value ? data.value.now * 1000 + Math.max(0, tick.value - readAt.value) : tick.value,
)

const zone = computed(() => draft.zone || 'UTC')
const shownTime = computed(() => {
  try {
    return new Intl.DateTimeFormat(locale.value, {
      dateStyle: 'full',
      timeStyle: 'medium',
      timeZone: zone.value,
    }).format(nasNow.value)
  } catch {
    return new Intl.DateTimeFormat(locale.value, { dateStyle: 'full', timeStyle: 'medium' }).format(
      nasNow.value,
    )
  }
})
/** The date-time field: what was typed, this computer's time, or the NAS's. */
const clockField = computed({
  get: () =>
    draft.source === 'typed'
      ? draft.clock
      : toWallTime(draft.source === 'browser' ? tick.value : nasNow.value, zone.value),
  set: (value: string) => {
    draft.clock = value
    draft.source = 'typed'
  },
})

function apply(): void {
  let clock: number | undefined
  if (draft.mode === 'hand' && draft.source === 'browser') clock = Math.round(Date.now() / 1000)
  if (draft.mode === 'hand' && draft.source === 'typed') {
    const at = fromWallTime(draft.clock, zone.value)
    if (at !== null) clock = Math.round(at / 1000)
  }
  setTime.mutate(
    { zone: draft.zone, network_time: draft.mode === 'servers', servers: servers.value, clock },
    { onSuccess: (state) => reset(state) },
  )
}
</script>

<!-- The NAS's time zone, and whether time servers keep its clock or it is set by hand. -->
<template>
  <div v-if="isPending" class="h-64 max-w-xl animate-pulse rounded-lg bg-muted" />
  <template v-else-if="data">
    <PageDescription>
      <p>{{ t('clock.explanation') }}</p>
      <p>{{ t('clock.serversExplanation') }}</p>
    </PageDescription>
    <div class="grid max-w-xl gap-7">
      <div class="grid gap-2">
        <Label for="clock-zone">{{ t('clock.zone') }}</Label>
        <NativeSelect id="clock-zone" v-model="draft.zone" :disabled="!canManage">
          <option v-for="entry in data.zones" :key="entry" :value="entry">{{ entry }}</option>
        </NativeSelect>
        <p class="text-sm text-muted-foreground">
          {{ t('clock.now') }}:
          <span class="font-medium text-foreground tabular-nums">{{ shownTime }}</span>
        </p>
      </div>

      <div class="grid gap-2.5">
        <Label>{{ t('clock.mode') }}</Label>
        <RadioGroup v-model="draft.mode" :label="t('clock.mode')" :disabled="!canManage">
          <RadioGroupItem value="servers" :disabled="!data.can_network_time">
            {{ t('clock.fromServers') }}
          </RadioGroupItem>
          <RadioGroupItem value="hand">{{ t('clock.byHand') }}</RadioGroupItem>
        </RadioGroup>
        <p v-if="!data.can_network_time" class="text-sm text-muted-foreground">
          {{ t('clock.noTimeService') }}
        </p>
      </div>

      <div v-if="draft.mode === 'servers'" class="grid gap-2">
        <Label>{{ t('clock.servers') }}</Label>
        <div class="grid gap-2">
          <div v-for="(_, index) in draft.servers" :key="index" class="flex gap-2">
            <Input
              v-model="draft.servers[index]"
              :disabled="!canManage"
              :placeholder="t('clock.serverPlaceholder')"
              :aria-label="`${t('clock.servers')} ${index + 1}`"
              autocomplete="off"
              spellcheck="false"
              class="font-mono text-dense"
            />
            <Button
              variant="ghost"
              size="icon"
              :disabled="!canManage"
              :aria-label="t('clock.removeServer', { server: draft.servers[index] || index + 1 })"
              @click="draft.servers.splice(index, 1)"
            >
              <X />
            </Button>
          </div>
          <Button
            v-if="draft.servers.length < MAX_SERVERS"
            variant="outline"
            size="sm"
            class="w-fit"
            :disabled="!canManage"
            @click="draft.servers.push('')"
          >
            <Plus />
            {{ t('clock.addServer') }}
          </Button>
        </div>
        <p class="text-sm text-muted-foreground">{{ t('clock.serversDescription') }}</p>
        <p
          v-if="data.network_time"
          class="flex items-center gap-1.5 text-sm"
          :class="!data.synchronized && 'text-muted-foreground'"
        >
          <CircleCheck v-if="data.synchronized" class="size-4 text-success" />
          <CircleDashed v-else class="size-4" />
          <template v-if="data.synchronized && data.server_in_use">
            {{ t('clock.synchronizedWith') }}
            <span class="font-mono text-dense">{{ data.server_in_use }}</span>
          </template>
          <template v-else-if="data.synchronized">{{ t('clock.synchronized') }}</template>
          <template v-else>{{ t('clock.notSynchronized') }}</template>
        </p>
      </div>

      <div v-else class="grid gap-2">
        <Label for="clock-time">{{ t('clock.dateTime') }}</Label>
        <div class="flex flex-wrap gap-2">
          <Input
            id="clock-time"
            v-model="clockField"
            type="datetime-local"
            step="1"
            :disabled="!canManage"
            class="w-auto font-mono text-dense tabular-nums"
          />
          <Button
            variant="outline"
            :disabled="!canManage"
            :aria-pressed="draft.source === 'browser'"
            @click="draft.source = 'browser'"
          >
            <Monitor />
            {{ t('clock.useBrowserTime') }}
          </Button>
        </div>
        <p class="text-sm text-muted-foreground">
          {{
            draft.source === 'browser'
              ? t('clock.browserTimeDescription')
              : t('clock.dateTimeDescription')
          }}
        </p>
      </div>

      <p v-if="!canManage" class="text-sm text-muted-foreground">{{ t('clock.viewOnly') }}</p>
    </div>

    <PageFooter
      :dirty="dirty"
      :saving="setTime.isPending.value"
      :error="setTime.error.value ? problemMessage(setTime.error.value) : null"
      @reset="reset()"
      @apply="apply"
    />
  </template>
</template>
