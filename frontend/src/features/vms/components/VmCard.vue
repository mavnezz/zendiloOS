<script setup lang="ts">
import {
  CirclePower,
  LoaderCircle,
  Monitor,
  Pencil,
  Play,
  Power,
  RotateCcw,
  Square,
  Trash2,
  Zap,
} from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmLoad } from '@/shared/api/types'
import { formatBytes, formatPercent, formatRate } from '@/shared/lib/format'
import { ActionMenu, type Action } from '@/shared/ui/action-menu'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { Meter } from '@/shared/ui/meter'
import { SplitButton } from '@/shared/ui/split-button'

import {
  type CardAction,
  formatMemory,
  fromNetwork,
  isOn,
  stateVariant,
  totalDiskGib,
} from '../lib/vms'

/** One VM; `load`: what it uses now, while it runs and is looked at. */
const props = defineProps<{ vm: VirtualMachine; load?: VmLoad | null; busy?: boolean }>()
const emit = defineEmits<{
  console: []
  start: []
  shutDown: []
  action: [id: CardAction]
}>()

const { t } = useI18n()
const on = computed(() => isOn(props.vm.state))
const creating = computed(() => props.vm.state === 'creating')

const stops = computed<Action[]>(() => [
  { id: 'restart', label: t('vms.restart'), icon: RotateCcw },
  { id: 'force_off', label: t('vms.forceOff'), icon: Zap, destructive: true },
])

const more = computed<Action[]>(() => [
  { id: 'edit', label: t('vms.editVm'), icon: Pencil, disabled: creating.value },
  {
    id: 'autostart',
    label: props.vm.autostart ? t('vms.autostartOff') : t('vms.autostartOn'),
    icon: CirclePower,
    disabled: creating.value,
  },
  {
    id: 'delete',
    label: t('vms.delete'),
    icon: Trash2,
    destructive: true,
    disabled: creating.value,
  },
])

const network = computed(() => {
  if (!props.vm.nics.length) return t('vms.noNetwork')
  const ways = props.vm.nics.map((nic) => {
    const { kind, device } = fromNetwork(nic.network)
    return kind === 'user' ? t('vms.userNetwork') : t(`vms.networks.on.${kind}`, { device })
  })
  return [...new Set(ways)].join(', ')
})

const memoryBytes = computed(() => props.vm.memory_mib * 1024 * 1024)
const used = computed(() => props.load?.memory_used_bytes ?? null)
const counted = computed(
  () =>
    props.load?.received_bytes_per_second !== null &&
    props.load?.received_bytes_per_second !== undefined,
)
</script>

<!-- One VM: what it is, what it uses, and what can be done with it. -->
<template>
  <article class="grid gap-4 rounded-xl border bg-card p-4" :data-vm="vm.id">
    <header class="flex items-start gap-3">
      <span
        class="grid size-10 shrink-0 place-items-center rounded-lg bg-linear-to-br from-violet-400 to-indigo-600 text-white"
        aria-hidden="true"
      >
        <Monitor class="size-5" />
      </span>
      <div class="grid min-w-0 flex-1 gap-0.5">
        <h3 class="truncate font-semibold" :title="vm.name">{{ vm.name }}</h3>
        <p class="truncate text-xs text-muted-foreground">
          {{ t(`vms.os.${vm.os}`) }} · {{ network }}
        </p>
      </div>
      <Badge :variant="stateVariant(vm.state)" data-testid="vm-state">
        <LoaderCircle v-if="creating || vm.state === 'shutting_down'" class="animate-spin" />
        {{ t(`vms.states.${vm.state}`) }}
      </Badge>
    </header>

    <dl v-if="on" class="grid gap-3 text-sm" data-testid="vm-load">
      <div class="grid gap-1">
        <div class="flex items-baseline justify-between gap-2">
          <dt class="text-xs text-muted-foreground">
            {{ t('vms.load.cpu') }} · {{ t('vms.cpus', vm.cpus) }}
          </dt>
          <dd class="font-medium tabular-nums">
            {{
              load?.cpu_percent !== null && load?.cpu_percent !== undefined
                ? formatPercent(load.cpu_percent)
                : t('vms.load.waiting')
            }}
          </dd>
        </div>
        <Meter :value="(load?.cpu_percent ?? 0) / 100" :label="t('vms.load.cpu')" />
      </div>
      <div class="grid gap-1">
        <div class="flex items-baseline justify-between gap-2">
          <dt class="text-xs text-muted-foreground">{{ t('vms.load.memory') }}</dt>
          <dd class="font-medium tabular-nums">
            {{
              used !== null
                ? t('vms.overview.memoryValue', {
                    assigned: formatBytes(used),
                    total: formatMemory(vm.memory_mib),
                  })
                : formatMemory(vm.memory_mib)
            }}
          </dd>
        </div>
        <Meter :value="used !== null ? used / memoryBytes : 0" :label="t('vms.load.memory')" />
      </div>
      <div class="flex items-baseline justify-between gap-2">
        <dt class="text-xs text-muted-foreground">{{ t('vms.load.network') }}</dt>
        <dd v-if="counted && load" class="flex gap-3 font-medium tabular-nums">
          <span>↓ {{ formatRate(load.received_bytes_per_second ?? 0) }}</span>
          <span>↑ {{ formatRate(load.sent_bytes_per_second ?? 0) }}</span>
        </dd>
        <dd v-else class="text-xs text-muted-foreground">
          {{ load ? t('vms.load.notCounted') : t('vms.load.waiting') }}
        </dd>
      </div>
    </dl>
    <dl v-else class="grid grid-cols-3 gap-2 text-sm">
      <div class="grid gap-0.5">
        <dt class="text-xs text-muted-foreground">CPU</dt>
        <dd class="font-medium tabular-nums">{{ t('vms.cpus', vm.cpus) }}</dd>
      </div>
      <div class="grid gap-0.5">
        <dt class="text-xs text-muted-foreground">{{ t('vms.memory') }}</dt>
        <dd class="font-medium tabular-nums">{{ formatMemory(vm.memory_mib) }}</dd>
      </div>
      <div class="grid gap-0.5">
        <dt class="text-xs text-muted-foreground">{{ t('vms.disks', vm.disks.length) }}</dt>
        <dd class="font-medium tabular-nums">{{ totalDiskGib(vm) }} GiB</dd>
      </div>
    </dl>

    <footer class="flex flex-wrap items-center gap-2">
      <Button size="sm" :disabled="!on" @click="emit('console')">
        <Monitor />
        {{ t('vms.console') }}
      </Button>
      <SplitButton
        v-if="on"
        :label="t('vms.shutDown')"
        :icon="Square"
        :actions="stops"
        :menu-label="t('vms.shutDownMenu', { name: vm.name })"
        :disabled="busy || vm.state === 'shutting_down'"
        @click="emit('shutDown')"
        @select="(id: string) => emit('action', id as CardAction)"
      />
      <Button
        v-else
        size="sm"
        variant="outline"
        :disabled="busy || creating"
        @click="emit('start')"
      >
        <Play />
        {{ t('vms.start') }}
      </Button>
      <Badge v-if="vm.autostart" variant="outline" class="ml-1">
        <Power />
        {{ t('vms.autostart') }}
      </Badge>
      <span class="ml-auto">
        <ActionMenu
          :actions="more"
          :label="t('vms.more', { name: vm.name })"
          @select="(id: string) => emit('action', id as CardAction)"
        />
      </span>
    </footer>
  </article>
</template>
