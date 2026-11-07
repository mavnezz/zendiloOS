<script setup lang="ts">
import {
  ArrowLeft,
  ArrowUpCircle,
  Box,
  LoaderCircle,
  Play,
  RotateCw,
  Square,
} from 'lucide-vue-next'
import { computed, onBeforeUnmount, ref, toRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { PageTabs } from '@/shared/ui/page'

import { useContainerAction, useContainerDetails, useUpdatable } from '../api'
import { isUp } from '../lib/status'
import { useContainerUpdates } from '../updating'
import ContainerLog from './ContainerLog.vue'
import ContainerOverview from './ContainerOverview.vue'
import ContainerProcesses from './ContainerProcesses.vue'
import ContainerTerminal from './ContainerTerminal.vue'
import UpdateDialog from './UpdateDialog.vue'

const props = defineProps<{ id: string }>()
const emit = defineEmits<{ back: [] }>()

const { t } = useI18n()
const id = toRef(props, 'id')
const { data: details, error } = useContainerDetails(id)
const container = computed(() => details.value?.container)
const running = computed(() => Boolean(container.value && isUp(container.value)))
const act = useContainerAction()
const updatable = useUpdatable()
const updates = useContainerUpdates()

const tab = ref('overview')
const tabs = computed(() =>
  ['overview', 'processes', 'log', 'terminal'].map((tabId) => ({
    id: tabId,
    label: t(`docker.details.sections.${tabId}`),
  })),
)

// The uptime counts on between the asks.
const now = ref(Date.now())
const ticker = window.setInterval(() => (now.value = Date.now()), 1000)
onBeforeUnmount(() => window.clearInterval(ticker))

function run(action: 'start' | 'stop' | 'restart'): void {
  act.mutate({ id: props.id, action })
}
</script>

<!-- One container: what it is, what runs in it, what it writes, and a terminal in it. -->
<template>
  <div class="flex min-h-full flex-col">
    <div class="mb-4 flex flex-wrap items-center gap-3">
      <Button variant="ghost" size="icon" :aria-label="t('docker.back')" @click="emit('back')">
        <ArrowLeft />
      </Button>
      <span class="grid size-12 place-items-center rounded-xl bg-primary/10 text-primary">
        <Box class="size-6" />
      </span>
      <div class="grid min-w-0 flex-1 gap-0.5">
        <span class="flex flex-wrap items-center gap-2">
          <span class="text-lg font-semibold">{{ container?.name ?? '…' }}</span>
          <Badge v-if="container" :variant="running ? 'success' : 'secondary'">{{
            t(`docker.status.${running ? 'running' : 'stopped'}`)
          }}</Badge>
          <Badge
            v-if="container?.health"
            :variant="container.health === 'healthy' ? 'success' : 'warning'"
            >{{ container.health }}</Badge
          >
          <Badge v-if="container && updates.updating.value.has(container.name)" variant="info">
            <LoaderCircle class="size-3 animate-spin" />
            {{ t('docker.update.running') }}
          </Badge>
          <button
            v-else-if="container && updatable.has(container.name)"
            type="button"
            class="rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring"
            @click="updates.ask(container)"
          >
            <Badge variant="info">
              <ArrowUpCircle class="size-3" />
              {{ t('docker.updatable') }}
            </Badge>
          </button>
        </span>
        <span v-if="container" class="truncate text-xs text-muted-foreground">
          {{ t('docker.details.createdAt', { at: formatDateTime(container.created_at * 1000) }) }}
          · <span class="font-mono">{{ container.image }}</span>
        </span>
      </div>
      <Button
        variant="ghost"
        size="sm"
        :disabled="!container || running || act.isPending.value"
        @click="run('start')"
      >
        <Play />
        {{ t('docker.actions.start') }}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        :disabled="!running || act.isPending.value"
        @click="run('stop')"
      >
        <Square />
        {{ t('docker.actions.stop') }}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        :disabled="!running || act.isPending.value"
        @click="run('restart')"
      >
        <RotateCw />
        {{ t('docker.actions.restart') }}
      </Button>
    </div>
    <Alert
      v-if="error || act.error.value || updates.failure.value"
      variant="destructive"
      class="mb-3"
      >{{ problemMessage(error ?? act.error.value ?? updates.failure.value) }}</Alert
    >
    <PageTabs v-model="tab" :tabs="tabs" :label="container?.name ?? id" />

    <template v-if="details">
      <ContainerOverview v-if="tab === 'overview'" :details="details" :now="now" />
      <ContainerProcesses v-else-if="tab === 'processes'" :id="id" :running="running" />
      <ContainerLog v-else-if="tab === 'log'" :id="id" :name="details.container.name" />
      <!-- Kept while other tabs are shown: its sessions stay open. -->
      <ContainerTerminal v-show="tab === 'terminal'" :container="id" :running="running" />
    </template>
    <div v-else-if="!error" class="h-40 animate-pulse rounded-xl bg-muted" />

    <UpdateDialog
      :open="updates.asking.value !== null"
      :container="updates.asking.value"
      @update:open="(open: boolean) => !open && (updates.asking.value = null)"
      @confirm="updates.asking.value && updates.start(updates.asking.value)"
    />
  </div>
</template>
