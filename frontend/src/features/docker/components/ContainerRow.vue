<script setup lang="ts">
import { ArrowUpCircle, Box, LoaderCircle } from 'lucide-vue-next'
import { computed, inject } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerContainer } from '@/shared/api/types'
import { formatBytes, formatRate } from '@/shared/lib/format'
import { Badge } from '@/shared/ui/badge'

import { OPEN_CONTAINER } from '../lib/navigation'
import { isUp } from '../lib/status'
import { imageParts } from '../lib/usage'

/** `updating`: an update of it runs now. */
const props = defineProps<{
  container: DockerContainer
  showProject?: boolean
  updatable?: boolean
  /** What its network sends and receives now, per second, instead of the totals. */
  rates?: { sent: number; received: number }
  updating?: boolean
  /**
   * The name is no link of its own: inside something that opens the
   * container already (a project's row), so one click takes one way.
   */
  unlinked?: boolean
}>()
const emit = defineEmits<{ update: [] }>()
const { t } = useI18n()
/** Within the Docker app, the name opens the container's page. */
const open = inject(OPEN_CONTAINER, null)

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
          <button
            v-if="open && !unlinked"
            type="button"
            class="truncate rounded font-medium outline-none hover:text-primary hover:underline focus-visible:ring-2 focus-visible:ring-ring"
            @click="open(container.name)"
          >
            {{ container.name }}
          </button>
          <span v-else class="truncate font-medium">{{ container.name }}</span>
          <Badge :variant="state === 'running' ? 'success' : 'secondary'">{{
            t(`docker.status.${state}`)
          }}</Badge>
          <Badge v-if="updating" variant="info">
            <LoaderCircle class="size-3 animate-spin" />
            {{ t('docker.update.running') }}
          </Badge>
          <button
            v-else-if="updatable"
            type="button"
            class="rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring"
            :title="t('docker.update.action')"
            @click="emit('update')"
          >
            <Badge variant="info">
              <ArrowUpCircle class="size-3" />
              {{ t('docker.updatable') }}
            </Badge>
          </button>
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
      <template v-if="rates">
        <span class="text-muted-foreground">↑ {{ formatRate(rates.sent) }}</span>
        <span class="text-muted-foreground">↓ {{ formatRate(rates.received) }}</span>
      </template>
      <template v-else>
        <span class="text-muted-foreground">↑ {{ formatBytes(usage.sent_bytes) }}</span>
        <span class="text-muted-foreground">↓ {{ formatBytes(usage.received_bytes) }}</span>
      </template>
    </div>
  </div>
</template>
