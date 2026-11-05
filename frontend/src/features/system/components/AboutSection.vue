<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { formatBytes } from '@/shared/lib/format'

import { useSystemInfo } from '../api'

const { t } = useI18n()
const { data: info } = useSystemInfo()
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
    <dt class="text-muted-foreground">{{ t('dashboard.processor') }}</dt>
    <dd>{{ info.cpu_model ?? '–' }} · {{ t('dashboard.threads', { count: info.cpu_threads }) }}</dd>
    <dt class="text-muted-foreground">{{ t('dashboard.memory') }}</dt>
    <dd>{{ formatBytes(info.memory_total_bytes) }}</dd>
  </dl>
</template>
