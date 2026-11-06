<script setup lang="ts">
import { Box } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerContainer } from '@/shared/api/types'
import { formatBytes } from '@/shared/lib/format'
import { Badge } from '@/shared/ui/badge'

import { isUp } from '../lib/status'
import { imageParts } from '../lib/usage'

const props = defineProps<{
  container: DockerContainer
  showProject?: boolean
  updatable?: boolean
}>()
const { t } = useI18n()

const image = computed(() => imageParts(props.container.image))
const usage = computed(() => props.container.usage)
const memoryShare = computed(() =>
  usage.value && usage.value.memory_limit_bytes
    ? usage.value.memory_bytes / usage.value.memory_limit_bytes
    : 0,
)
const state = computed(() => (isUp(props.container) ? 'running' : 'stopped'))
</script>

<!-- One container: what it is made from and, while it runs, what it uses. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-3">
      <span class="grid size-10 shrink-0 place-items-center rounded-lg bg-primary/10 text-primary">
        <Box class="size-5" />
      </span>
      <div class="grid min-w-0 flex-1 gap-0.5">
        <span class="flex flex-wrap items-center gap-2">
          <span class="truncate font-medium">{{ container.name }}</span>
          <Badge :variant="state === 'running' ? 'success' : 'secondary'">{{
            t(`docker.status.${state}`)
          }}</Badge>
          <Badge v-if="updatable" variant="info">{{ t('docker.updatable') }}</Badge>
          <Badge
            v-if="container.health"
            :variant="container.health === 'healthy' ? 'success' : 'warning'"
            >{{ container.health }}</Badge
          >
        </span>
        <span class="flex flex-wrap gap-x-3 text-xs text-muted-foreground">
          <span class="truncate">{{ t('docker.containers.image') }}: {{ image.name }}</span>
          <span>{{ t('docker.containers.tag') }}: {{ image.tag }}</span>
          <span v-if="showProject && container.project"
            >{{ t('docker.containers.project') }}: {{ container.project }}</span
          >
          <span
            v-for="port in container.ports"
            :key="`${port.host_port}/${port.protocol}`"
            class="font-mono"
          >
            {{ port.host_port }}→{{ port.container_port }}/{{ port.protocol }}
          </span>
        </span>
      </div>
      <slot />
    </div>
    <div v-if="usage" class="grid grid-cols-2 gap-x-6 gap-y-1 text-xs @2xl:grid-cols-4">
      <div class="grid gap-1">
        <span class="flex justify-between"
          ><span>CPU</span
          ><span class="tabular-nums">{{ usage.cpu_percent.toFixed(1) }} %</span></span
        >
        <span class="h-1 overflow-hidden rounded-full bg-muted">
          <span
            class="block h-full bg-chart-1"
            :style="{ width: `${Math.min(100, usage.cpu_percent)}%` }"
          />
        </span>
      </div>
      <div class="grid gap-1">
        <span class="flex justify-between"
          ><span>RAM</span
          ><span class="tabular-nums">{{ formatBytes(usage.memory_bytes) }}</span></span
        >
        <span class="h-1 overflow-hidden rounded-full bg-muted">
          <span class="block h-full bg-chart-5" :style="{ width: `${memoryShare * 100}%` }" />
        </span>
      </div>
      <span class="text-muted-foreground">↑ {{ formatBytes(usage.sent_bytes) }}</span>
      <span class="text-muted-foreground">↓ {{ formatBytes(usage.received_bytes) }}</span>
    </div>
  </div>
</template>
