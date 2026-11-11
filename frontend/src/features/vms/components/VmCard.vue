<script setup lang="ts">
import {
  CirclePower,
  LoaderCircle,
  Monitor,
  Play,
  Power,
  RotateCcw,
  Square,
  Trash2,
  Zap,
} from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine } from '@/shared/api/types'
import { ActionMenu, type Action } from '@/shared/ui/action-menu'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'

import { formatMemory, isOn, stateVariant } from '../lib/vms'

const props = defineProps<{ vm: VirtualMachine; busy?: boolean }>()
const emit = defineEmits<{
  console: []
  start: []
  shutDown: []
  action: [id: 'force_off' | 'restart' | 'autostart' | 'delete']
}>()

const { t } = useI18n()
const on = computed(() => isOn(props.vm.state))
const creating = computed(() => props.vm.state === 'creating')

const actions = computed<Action[]>(() => [
  { id: 'restart', label: t('vms.restart'), icon: RotateCcw, disabled: !on.value },
  { id: 'force_off', label: t('vms.forceOff'), icon: Zap, disabled: !on.value },
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

const network = computed(() =>
  props.vm.network.kind === 'bridge'
    ? t('vms.bridgeNetwork', { bridge: props.vm.network.bridge })
    : t('vms.userNetwork'),
)
</script>

<!-- One VM: what it is, what it does, and what can be done with it. -->
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

    <dl class="grid grid-cols-3 gap-2 text-sm">
      <div class="grid gap-0.5">
        <dt class="text-xs text-muted-foreground">CPU</dt>
        <dd class="font-medium tabular-nums">{{ t('vms.cpus', vm.cpus) }}</dd>
      </div>
      <div class="grid gap-0.5">
        <dt class="text-xs text-muted-foreground">{{ t('vms.memory') }}</dt>
        <dd class="font-medium tabular-nums">{{ formatMemory(vm.memory_mib) }}</dd>
      </div>
      <div class="grid gap-0.5">
        <dt class="text-xs text-muted-foreground">{{ t('vms.disk') }}</dt>
        <dd class="font-medium tabular-nums">{{ vm.disk_gib }} GiB</dd>
      </div>
    </dl>

    <footer class="flex flex-wrap items-center gap-2">
      <Button size="sm" variant="secondary" :disabled="!on" @click="emit('console')">
        <Monitor />
        {{ t('vms.console') }}
      </Button>
      <Button
        v-if="on"
        size="sm"
        variant="outline"
        :disabled="busy || vm.state === 'shutting_down'"
        @click="emit('shutDown')"
      >
        <Square />
        {{ t('vms.shutDown') }}
      </Button>
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
          :actions="actions"
          :label="t('vms.more', { name: vm.name })"
          @select="
            (id: string) => emit('action', id as 'force_off' | 'restart' | 'autostart' | 'delete')
          "
        />
      </span>
    </footer>
  </article>
</template>
