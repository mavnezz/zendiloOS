<script setup lang="ts">
import { MonitorCog, Plus } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmPlugin } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useOpenApp } from '@/shared/lib/window-section'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

import { useVmActions, useVmImages, useVms, useVmStats } from '../api'
import CreateVmDialog from './CreateVmDialog.vue'
import DeleteVmDialog from './DeleteVmDialog.vue'
import EditVmDialog from './EditVmDialog.vue'
import type { CardAction } from '../lib/vms'
import VmCard from './VmCard.vue'
import VmsOverview from './VmsOverview.vue'

/** The VMs, their load, and what is done with them; `library` opens the images. */
defineProps<{ plugin: VmPlugin }>()
const emit = defineEmits<{ library: [] }>()

const { t } = useI18n()
const openApp = useOpenApp()
const actions = useVmActions()
const { data: vms, error, isPending } = useVms()
const { data: images } = useVmImages()
const stats = useVmStats()

const creating = ref(false)
const editing = ref<VirtualMachine | null>(null)
const deleting = ref<VirtualMachine | null>(null)
const forcing = ref<VirtualMachine | null>(null)
const busy = ref<string | null>(null)
const failure = ref<string | null>(null)

const taken = computed(() => (vms.value ?? []).map((vm) => vm.name))
const othersOf = (vm: VirtualMachine) => taken.value.filter((name) => name !== vm.name)
const loadOf = (vm: VirtualMachine) => stats.value?.vms.find((load) => load.id === vm.id) ?? null

/** Runs a change of one VM, saying why it failed. */
async function act(vm: VirtualMachine, change: (id: string) => Promise<unknown>): Promise<void> {
  busy.value = vm.id
  failure.value = null
  try {
    await change(vm.id)
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = null
  }
}

function menu(vm: VirtualMachine, id: CardAction): void {
  if (id === 'force_off') forcing.value = vm
  else if (id === 'restart') void act(vm, actions.restart)
  else if (id === 'edit') editing.value = vm
  else if (id === 'autostart') void act(vm, (vmId) => actions.setAutostart(vmId, !vm.autostart))
  else deleting.value = vm
}

function forceOff(): void {
  const vm = forcing.value
  if (vm) void act(vm, actions.forceOff)
}
</script>

<!-- The machines: how they do, a card each, and making one. -->
<template>
  <div class="grid content-start gap-6">
    <header class="flex flex-wrap items-center gap-3">
      <div class="grid min-w-0 flex-1 gap-0.5">
        <h2 class="text-xl font-semibold">{{ t('vms.sections.machines') }}</h2>
        <p v-if="plugin.storage" class="text-sm text-muted-foreground">
          {{
            t('appStore.placeVolume', { number: plugin.storage.volume, pool: plugin.storage.pool })
          }}
        </p>
      </div>
      <Button v-if="vms?.length" @click="creating = true">
        <Plus />
        {{ t('vms.create') }}
      </Button>
    </header>

    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>

    <div v-if="isPending" class="h-40 animate-pulse rounded-xl bg-muted" />
    <div
      v-else-if="!vms?.length"
      class="grid justify-items-center gap-3 rounded-xl border border-dashed p-10 text-center"
      data-testid="vms-empty"
    >
      <MonitorCog class="size-10 text-muted-foreground" />
      <p class="font-medium">{{ t('vms.empty') }}</p>
      <p class="max-w-md text-sm text-muted-foreground">{{ t('vms.emptyHint') }}</p>
      <div class="flex flex-wrap justify-center gap-2">
        <Button @click="creating = true">
          <Plus />
          {{ t('vms.create') }}
        </Button>
        <Button variant="outline" @click="emit('library')">
          {{ t('vms.devices.toImages') }}
        </Button>
      </div>
    </div>
    <template v-else>
      <VmsOverview :vms="vms" :plugin="plugin" :stats="stats" />
      <ul class="grid gap-4 @2xl:grid-cols-2 @5xl:grid-cols-3">
        <li v-for="vm in vms" :key="vm.id">
          <VmCard
            :vm="vm"
            :load="loadOf(vm)"
            :busy="busy === vm.id"
            @console="openApp?.('vm-console', vm.id)"
            @start="act(vm, actions.start)"
            @shut-down="act(vm, actions.shutDown)"
            @action="(id) => menu(vm, id)"
          />
        </li>
      </ul>
    </template>

    <CreateVmDialog
      v-model:open="creating"
      :plugin="plugin"
      :taken="taken"
      :images="images ?? []"
      @library="emit('library')"
    />
    <EditVmDialog
      v-if="editing"
      :vm="editing"
      :plugin="plugin"
      :taken="othersOf(editing)"
      :images="images ?? []"
      :open="Boolean(editing)"
      @update:open="(open: boolean) => !open && (editing = null)"
      @library="emit('library')"
    />
    <DeleteVmDialog
      v-if="deleting"
      :vm="deleting"
      :open="Boolean(deleting)"
      @update:open="(open: boolean) => !open && (deleting = null)"
    />
    <ConfirmDialog
      :open="Boolean(forcing)"
      :title="t('vms.forceOffTitle', { name: forcing?.name ?? '' })"
      :description="t('vms.forceOffText')"
      :confirm-label="t('vms.forceOff')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (forcing = null)"
      @confirm="forceOff"
    />
  </div>
</template>
