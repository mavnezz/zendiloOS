<script setup lang="ts">
import { Cpu, MemoryStick, MonitorCog } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmPlugin, VmStats } from '@/shared/api/types'
import { formatBytes, formatPercent } from '@/shared/lib/format'
import { Meter } from '@/shared/ui/meter'

import { isOn } from '../lib/vms'

/**
 * The VMs at a glance: how many run, the share of the NAS's CPU they keep
 * busy, and the memory the running ones were given against the NAS's.
 */
const props = defineProps<{ vms: VirtualMachine[]; plugin: VmPlugin; stats: VmStats | null }>()

const { t } = useI18n()

const running = computed(
  () => props.stats?.running ?? props.vms.filter((vm) => isOn(vm.state)).length,
)
const assigned = computed(
  () =>
    props.stats?.memory_assigned_bytes ??
    props.vms
      .filter((vm) => isOn(vm.state))
      .reduce((total, vm) => total + vm.memory_mib * 1024 * 1024, 0),
)
const total = computed(() => props.stats?.host_memory_bytes || props.plugin.memory_bytes)
</script>

<template>
  <section class="grid gap-3 sm:grid-cols-3" data-testid="vms-overview">
    <div class="grid gap-1 rounded-xl border bg-card p-4">
      <span class="flex items-center gap-2 text-xs text-muted-foreground">
        <MonitorCog class="size-4" />
        {{ t('vms.overview.running') }}
      </span>
      <span class="text-xl font-semibold tabular-nums">
        {{ t('vms.overview.runningValue', { running, total: vms.length }) }}
      </span>
    </div>
    <div class="grid gap-1.5 rounded-xl border bg-card p-4">
      <span class="flex items-center gap-2 text-xs text-muted-foreground">
        <Cpu class="size-4" />
        {{ t('vms.overview.cpu') }}
      </span>
      <span class="text-xl font-semibold tabular-nums">
        {{ stats ? formatPercent(stats.cpu_percent) : '–' }}
      </span>
      <Meter :value="(stats?.cpu_percent ?? 0) / 100" :label="t('vms.overview.cpu')" />
      <span class="text-xs text-muted-foreground">
        {{ t('vms.overview.cpuHint', { cores: stats?.host_cores || plugin.cores }) }}
      </span>
    </div>
    <div class="grid gap-1.5 rounded-xl border bg-card p-4">
      <span class="flex items-center gap-2 text-xs text-muted-foreground">
        <MemoryStick class="size-4" />
        {{ t('vms.overview.memory') }}
      </span>
      <span class="text-xl font-semibold tabular-nums">
        {{
          t('vms.overview.memoryValue', {
            assigned: formatBytes(assigned),
            total: formatBytes(total),
          })
        }}
      </span>
      <Meter :value="total ? assigned / total : 0" :label="t('vms.overview.memory')" />
      <span class="text-xs text-muted-foreground">{{ t('vms.overview.memoryHint') }}</span>
    </div>
  </section>
</template>
