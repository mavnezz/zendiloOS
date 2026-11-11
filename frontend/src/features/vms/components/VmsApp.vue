<script setup lang="ts">
import { MonitorCog, Plus } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useOpenApp } from '@/shared/lib/window-section'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

import { useVmActions, useVmPlugin, useVms } from '../api'
import CreateVmDialog from './CreateVmDialog.vue'
import DeleteVmDialog from './DeleteVmDialog.vue'
import VmCard from './VmCard.vue'

const { t } = useI18n()
const openApp = useOpenApp()
const actions = useVmActions()
const { data: plugin, error: pluginError } = useVmPlugin()
const ready = computed(() => Boolean(plugin.value?.installed && plugin.value.storage))
const { data: vms, error, isPending } = useVms(() => ready.value)

const creating = ref(false)
const deleting = ref<VirtualMachine | null>(null)
const forcing = ref<VirtualMachine | null>(null)
const busy = ref<string | null>(null)
const failure = ref<string | null>(null)

const taken = computed(() => (vms.value ?? []).map((vm) => vm.name))

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

function menu(vm: VirtualMachine, id: 'force_off' | 'restart' | 'autostart' | 'delete'): void {
  if (id === 'force_off') forcing.value = vm
  else if (id === 'restart') void act(vm, actions.restart)
  else if (id === 'autostart') void act(vm, (vmId) => actions.setAutostart(vmId, !vm.autostart))
  else deleting.value = vm
}

function forceOff(): void {
  const vm = forcing.value
  if (vm) void act(vm, actions.forceOff)
}
</script>

<!-- The Virtual machines app: a card per VM, and making one. -->
<template>
  <div class="grid content-start gap-6 p-6 @container">
    <header class="flex flex-wrap items-center gap-3">
      <div class="grid min-w-0 flex-1 gap-0.5">
        <h2 class="text-xl font-semibold">{{ t('apps.vms') }}</h2>
        <p v-if="plugin?.storage" class="text-sm text-muted-foreground">
          {{
            t('appStore.placeVolume', { number: plugin.storage.volume, pool: plugin.storage.pool })
          }}
        </p>
      </div>
      <Button v-if="ready && vms?.length" @click="creating = true">
        <Plus />
        {{ t('vms.create') }}
      </Button>
    </header>

    <Alert v-if="pluginError || error" variant="destructive">{{
      problemMessage(pluginError ?? error)
    }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>

    <div
      v-if="plugin && !ready"
      class="grid justify-items-start gap-3 rounded-xl border p-6 text-sm"
      data-testid="vms-not-ready"
    >
      <MonitorCog class="size-8 text-muted-foreground" />
      <p class="text-muted-foreground">{{ t('vms.notReady') }}</p>
      <Button v-if="openApp" size="sm" variant="outline" @click="openApp('apps', 'vms')">
        {{ t('vms.toAppCenter') }}
      </Button>
    </div>
    <div v-else-if="ready && isPending" class="h-40 animate-pulse rounded-xl bg-muted" />
    <div
      v-else-if="ready && !vms?.length"
      class="grid justify-items-center gap-3 rounded-xl border border-dashed p-10 text-center"
      data-testid="vms-empty"
    >
      <MonitorCog class="size-10 text-muted-foreground" />
      <p class="font-medium">{{ t('vms.empty') }}</p>
      <p class="max-w-md text-sm text-muted-foreground">{{ t('vms.emptyHint') }}</p>
      <Button @click="creating = true">
        <Plus />
        {{ t('vms.create') }}
      </Button>
    </div>
    <ul v-else-if="ready" class="grid gap-4 @2xl:grid-cols-2 @5xl:grid-cols-3">
      <li v-for="vm in vms" :key="vm.id">
        <VmCard
          :vm="vm"
          :busy="busy === vm.id"
          @console="openApp?.('vm-console', vm.id)"
          @start="act(vm, actions.start)"
          @shut-down="act(vm, actions.shutDown)"
          @action="(id) => menu(vm, id)"
        />
      </li>
    </ul>

    <CreateVmDialog
      v-if="plugin && ready"
      v-model:open="creating"
      :plugin="plugin"
      :taken="taken"
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
