<script setup lang="ts">
import { FileText, RotateCw, Search, Trash2 } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerContainer } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { type Action, ActionMenu } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Switch } from '@/shared/ui/switch'

import { useContainerAction, useContainers, useLogs, useUpdatable } from '../api'
import { isUp } from '../lib/status'
import ContainerRow from './ContainerRow.vue'
import LogView from './LogView.vue'

const { t } = useI18n()
const { data: containers, isPending } = useContainers()
const act = useContainerAction()
const updatable = useUpdatable()
const filter = ref('')
const removing = ref<DockerContainer | null>(null)
const reading = ref<DockerContainer | null>(null)
const readingId = computed(() => reading.value?.id ?? '')
const { data: logs, isPending: logsPending } = useLogs(
  'containers',
  readingId,
  () => reading.value !== null,
)

const shown = computed(() => {
  const term = filter.value.trim().toLowerCase()
  return (containers.value ?? []).filter(
    (c) => !term || c.name.toLowerCase().includes(term) || c.image.toLowerCase().includes(term),
  )
})

function actions(container: DockerContainer): Action[] {
  return [
    { id: 'logs', label: t('docker.containers.logs'), icon: FileText },
    {
      id: 'restart',
      label: t('docker.actions.restart'),
      icon: RotateCw,
      disabled: !isUp(container),
    },
    {
      id: 'remove',
      label: t('docker.actions.delete'),
      icon: Trash2,
      destructive: true,
      disabled: isUp(container),
    },
  ]
}

function run(container: DockerContainer, action: string): void {
  if (action === 'logs') reading.value = container
  else if (action === 'remove') removing.value = container
  else act.mutate({ id: container.id, action: 'restart' })
}

function toggle(container: DockerContainer, on: boolean): void {
  act.mutate({ id: container.id, action: on ? 'start' : 'stop' })
}
function confirmRemove(): void {
  if (removing.value) act.mutate({ id: removing.value.id, action: 'remove' })
  removing.value = null
}
</script>

<!-- Every container on the engine, whoever started it. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <div class="relative ml-auto w-full max-w-60">
        <Search class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="filter"
          class="pl-8"
          :placeholder="t('docker.search')"
          :aria-label="t('docker.search')"
        />
      </div>
    </div>
    <Alert v-if="act.error.value" variant="destructive">{{
      problemMessage(act.error.value)
    }}</Alert>
    <div v-if="isPending" class="h-40 animate-pulse rounded-xl bg-muted" />
    <p v-else-if="!shown.length" class="py-16 text-center text-sm text-muted-foreground">
      {{ t('docker.containers.none') }}
    </p>
    <ul class="grid gap-3">
      <li v-for="container in shown" :key="container.id" class="rounded-xl border bg-card p-4">
        <ContainerRow
          :container="container"
          show-project
          :updatable="updatable.has(container.name)"
        >
          <div class="flex items-center gap-1">
            <ActionMenu
              :actions="actions(container)"
              :label="t('docker.actions.more')"
              @select="run(container, $event)"
            />
            <Switch
              :model-value="isUp(container)"
              :aria-label="t('docker.containers.running', { name: container.name })"
              :disabled="act.isPending.value"
              @update:model-value="toggle(container, $event)"
            />
          </div>
        </ContainerRow>
      </li>
    </ul>
    <ConfirmDialog
      :open="removing !== null"
      :title="t('docker.containers.removeTitle', { name: removing?.name })"
      :description="t('docker.containers.removeText')"
      :confirm-label="t('docker.actions.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (removing = null)"
      @confirm="confirmRemove"
    />
    <ModalDialog
      :open="reading !== null"
      :title="t('docker.containers.logsTitle', { name: reading?.name })"
      :close-label="t('desktop.close')"
      wide
      @update:open="(open: boolean) => !open && (reading = null)"
    >
      <LogView :text="logs?.text" :loading="logsPending" />
    </ModalDialog>
  </div>
</template>
