<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { formatBytes, formatDuration } from '@/shared/lib/format'

import { useSystemInfo } from '../api'

const { t } = useI18n()
const { data: info } = useSystemInfo()

/** The machine's maker and model, if its firmware names them. */
function machine(vendor: string | null | undefined, product: string | null | undefined) {
  return [vendor, product].filter(Boolean).join(' ') || null
}
</script>

<template>
  <dl v-if="info" class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
    <dt class="text-muted-foreground">{{ t('settings.version') }}</dt>
    <dd class="font-medium">{{ info.version }}</dd>
    <dt class="text-muted-foreground">{{ t('dashboard.host') }}</dt>
    <dd>{{ info.hostname }}</dd>
    <dt class="text-muted-foreground">{{ t('dashboard.os') }}</dt>
    <dd>{{ info.os_name ?? 'Linux' }}</dd>
    <dt class="text-muted-foreground">{{ t('dashboard.kernel') }}</dt>
    <dd>{{ info.kernel_release }} ({{ info.architecture }})</dd>
    <dt class="text-muted-foreground">{{ t('dashboard.uptime') }}</dt>
    <dd>{{ formatDuration(info.uptime_seconds) }}</dd>
    <template v-if="machine(info.vendor, info.product)">
      <dt class="text-muted-foreground">{{ t('about.machine') }}</dt>
      <dd>{{ machine(info.vendor, info.product) }}</dd>
    </template>
    <template v-if="info.board">
      <dt class="text-muted-foreground">{{ t('about.board') }}</dt>
      <dd>{{ info.board }}</dd>
    </template>
    <template v-if="info.firmware">
      <dt class="text-muted-foreground">{{ t('about.firmware') }}</dt>
      <dd>{{ info.firmware }} · {{ info.uefi ? 'UEFI' : t('about.legacyBios') }}</dd>
    </template>
    <dt class="text-muted-foreground">{{ t('dashboard.processor') }}</dt>
    <dd>
      {{ info.cpu_model ?? '–' }} ·
      <template v-if="info.cpu_cores">{{ t('about.cores', { count: info.cpu_cores }) }}, </template>
      {{ t('dashboard.threads', { count: info.cpu_threads }) }}
    </dd>
    <dt class="text-muted-foreground">{{ t('dashboard.memory') }}</dt>
    <dd>
      {{ formatBytes(info.memory_total_bytes) }}
      <template v-if="info.swap_total_bytes > 0">
        · {{ t('about.swap', { size: formatBytes(info.swap_total_bytes) }) }}
      </template>
    </dd>
  </dl>
</template>
