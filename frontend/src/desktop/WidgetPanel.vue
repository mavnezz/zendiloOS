<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query'
import {
  ArrowDown,
  ArrowUp,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Cpu,
  Database,
  MemoryStick,
  Pencil,
  TriangleAlert,
} from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { useSessionStore } from '@/features/auth/store'
import { useNetwork } from '@/features/network/api'
import { useSystemInfo } from '@/features/system/api'
import { api, unwrap } from '@/shared/api/client'
import { formatBytes, formatDuration, formatPercent, formatRate } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'
import { usePreferences, WIDGETS, type WidgetId } from '@/shared/preferences/store'

import { memoryRatio, networkRates, useLatestMetrics } from './liveMetrics'

/** Load above which the NAS counts as busy rather than normal. */
const BUSY = 0.9

/** On a phone the panel spans the screen. */
defineProps<{ wide?: boolean }>()

const { t } = useI18n()
const session = useSessionStore()
const preferences = usePreferences()
const latest = useLatestMetrics()
const { data: info } = useSystemInfo()
const { data: network } = useNetwork()
const { data: pools } = useQuery({
  queryKey: ['storage', 'pools'],
  queryFn: () => unwrap(api.GET('/api/v1/storage/pools')),
  enabled: computed(() => session.can('storage.view')),
})
const editing = ref(false)

const memory = computed(() => (latest.value ? memoryRatio(latest.value) : 0))
const cpu = computed(() => (latest.value?.cpu_percent ?? 0) / 100)
const net = computed(() => (latest.value ? networkRates(latest.value) : null))
const busy = computed(() => cpu.value >= BUSY || memory.value >= BUSY)
const uptime = computed(() => latest.value?.uptime_seconds ?? info.value?.uptime_seconds)
/** The first IPv4 address of an interface that is up. */
const address = computed(
  () =>
    network.value?.interfaces
      .filter((item) => item.state === 'up')
      .flatMap((item) => item.addresses)
      .find((entry) => entry.address.includes('.') && entry.scope !== 'host')?.address,
)
const shownPools = computed(() =>
  (pools.value ?? []).filter((pool) => pool.used_bytes != null && pool.available_bytes != null),
)

const shown = computed(() => preferences.widgets)
const rest = computed(() => WIDGETS.filter((id) => !shown.value.includes(id)))

function toggle(id: WidgetId, on: boolean): void {
  preferences.chooseWidgets(on ? [...shown.value, id] : shown.value.filter((w) => w !== id))
}

function move(at: number, by: number): void {
  const next = [...shown.value]
  const [moved] = next.splice(at, 1)
  next.splice(at + by, 0, moved!)
  preferences.chooseWidgets(next)
}

const tone = (ratio: number) =>
  ratio >= 0.9 ? 'bg-destructive' : ratio >= 0.75 ? 'bg-warning' : 'bg-primary'
const widget = 'glass glass-strong rounded-panel p-3.5'
</script>

<!--
  Live readings along the right edge: is the NAS all right, and is it busy,
  without opening an app. Which ones appear, and in what order, belongs to
  the account.
-->
<template>
  <aside
    :aria-label="t('desktop.widgets')"
    :class="
      cn(
        'z-30 flex shrink-0 flex-col gap-3 overflow-y-auto motion-safe:animate-slide-in-right',
        wide ? 'w-full' : 'w-72',
      )
    "
  >
    <div v-if="editing" :class="cn(widget, 'grid gap-1')">
      <div v-for="id in [...shown, ...rest]" :key="id" class="flex items-center gap-2 text-sm">
        <input
          :id="`widget-${id}`"
          type="checkbox"
          class="size-4 accent-primary"
          :checked="shown.includes(id)"
          :aria-label="t('desktop.showWidget', { widget: t(`desktop.widget.${id}`) })"
          @change="toggle(id, ($event.target as HTMLInputElement).checked)"
        />
        <label :for="`widget-${id}`" class="min-w-0 flex-1 truncate">{{
          t(`desktop.widget.${id}`)
        }}</label>
        <button
          type="button"
          :disabled="!shown.includes(id) || shown.indexOf(id) === 0"
          :aria-label="t('desktop.moveUp')"
          class="rounded p-1 text-muted-foreground outline-none hover:bg-foreground/10 focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-30"
          @click="move(shown.indexOf(id), -1)"
        >
          <ChevronUp class="size-4" />
        </button>
        <button
          type="button"
          :disabled="!shown.includes(id) || shown.indexOf(id) === shown.length - 1"
          :aria-label="t('desktop.moveDown')"
          class="rounded p-1 text-muted-foreground outline-none hover:bg-foreground/10 focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-30"
          @click="move(shown.indexOf(id), 1)"
        >
          <ChevronDown class="size-4" />
        </button>
      </div>
    </div>
    <p v-else-if="!shown.length" :class="cn(widget, 'text-xs text-muted-foreground')">
      {{ t('desktop.noWidgets') }}
    </p>
    <template v-else>
      <template v-for="id in shown" :key="id">
        <section v-if="id === 'status'" :class="widget">
          <h3 class="mb-2.5 text-xs font-medium text-muted-foreground">
            {{ t('desktop.widget.status') }}
          </h3>
          <div class="flex items-center gap-2.5">
            <TriangleAlert v-if="busy" class="size-7 shrink-0 text-warning" />
            <CheckCircle2 v-else class="size-7 shrink-0 text-success" />
            <div class="min-w-0">
              <p class="text-sm font-medium">
                {{ busy ? t('desktop.systemBusy') : t('desktop.systemNormal') }}
              </p>
              <p class="truncate font-mono text-2xs text-muted-foreground">{{ info?.hostname }}</p>
            </div>
          </div>
          <dl class="mt-3 grid grid-cols-2 gap-2 border-t border-foreground/10 pt-3 text-xs">
            <div>
              <dt class="text-muted-foreground">{{ t('desktop.uptime') }}</dt>
              <dd class="font-medium tabular-nums">
                {{ uptime === undefined ? '–' : formatDuration(uptime) }}
              </dd>
            </div>
            <div class="min-w-0">
              <dt class="text-muted-foreground">{{ t('desktop.address') }}</dt>
              <dd class="truncate font-mono font-medium">{{ address ?? '–' }}</dd>
            </div>
          </dl>
        </section>

        <section v-else-if="id === 'resources'" :class="widget">
          <h3 class="mb-2.5 text-xs font-medium text-muted-foreground">
            {{ t('desktop.widget.resources') }}
          </h3>
          <div class="space-y-3">
            <div
              v-for="meter in [
                { icon: Cpu, label: 'CPU', ratio: cpu },
                { icon: MemoryStick, label: t('desktop.memory'), ratio: memory },
              ]"
              :key="meter.label"
              class="space-y-1.5"
            >
              <div class="flex items-baseline justify-between gap-2 text-xs">
                <span class="flex items-center gap-1.5 text-muted-foreground">
                  <component :is="meter.icon" class="size-3.5" />
                  {{ meter.label }}
                </span>
                <span class="font-medium tabular-nums">{{ formatPercent(meter.ratio * 100) }}</span>
              </div>
              <div class="h-1.5 overflow-hidden rounded-full bg-foreground/10">
                <div
                  :class="
                    cn('h-full rounded-full transition-[width] duration-500', tone(meter.ratio))
                  "
                  :style="{ width: `${Math.min(100, meter.ratio * 100)}%` }"
                />
              </div>
            </div>
          </div>
        </section>

        <section v-else-if="id === 'network'" :class="widget">
          <h3 class="mb-2.5 text-xs font-medium text-muted-foreground">
            {{ t('desktop.widget.network') }}
          </h3>
          <div class="grid grid-cols-2 gap-2 text-xs">
            <div class="flex items-center gap-1.5">
              <ArrowUp class="size-3.5 text-success" />
              <div>
                <div class="text-muted-foreground">{{ t('desktop.upload') }}</div>
                <div class="font-medium tabular-nums">{{ net ? formatRate(net.sent) : '–' }}</div>
              </div>
            </div>
            <div class="flex items-center gap-1.5">
              <ArrowDown class="size-3.5 text-primary" />
              <div>
                <div class="text-muted-foreground">{{ t('desktop.download') }}</div>
                <div class="font-medium tabular-nums">
                  {{ net ? formatRate(net.received) : '–' }}
                </div>
              </div>
            </div>
          </div>
        </section>

        <section v-else-if="id === 'storage' && shownPools.length" :class="widget">
          <h3 class="mb-2.5 text-xs font-medium text-muted-foreground">
            {{ t('desktop.widget.storage') }}
          </h3>
          <div class="space-y-3">
            <div v-for="pool in shownPools" :key="pool.guid" class="space-y-1.5">
              <div class="flex items-baseline justify-between gap-2 text-xs">
                <span class="flex items-center gap-1.5 text-muted-foreground">
                  <Database class="size-3.5" />
                  <span class="font-mono text-2xs">{{ pool.name }}</span>
                </span>
                <span class="font-medium tabular-nums">{{
                  formatBytes(pool.available_bytes ?? 0)
                }}</span>
              </div>
              <div class="h-1.5 overflow-hidden rounded-full bg-foreground/10">
                <div
                  :class="
                    cn(
                      'h-full rounded-full transition-[width] duration-500',
                      tone(
                        (pool.used_bytes ?? 0) /
                          ((pool.used_bytes ?? 0) + (pool.available_bytes ?? 0) || 1),
                      ),
                    )
                  "
                  :style="{
                    width: `${((pool.used_bytes ?? 0) / ((pool.used_bytes ?? 0) + (pool.available_bytes ?? 0) || 1)) * 100}%`,
                  }"
                />
              </div>
            </div>
          </div>
        </section>
      </template>
    </template>

    <button
      type="button"
      class="glass mx-auto flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-medium outline-none hover:bg-foreground/10 focus-visible:ring-2 focus-visible:ring-ring"
      @click="editing = !editing"
    >
      <Pencil class="size-3" />
      {{ editing ? t('desktop.done') : t('desktop.editWidgets') }}
    </button>
  </aside>
</template>
