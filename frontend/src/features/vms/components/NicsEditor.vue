<script setup lang="ts">
import { Network, Plus, Trash2 } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmNicModel, VmPlugin } from '@/shared/api/types'
import { useOpenApp } from '@/shared/lib/window-section'
import { Button } from '@/shared/ui/button'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'

import {
  NETWORK_KINDS,
  NIC_MODELS,
  NICS_MAX,
  addNic,
  networkChoices,
  removeNic,
  withNetworkKind,
  type NetworkKind,
  type VmForm,
} from '../lib/vms'

/**
 * A VM's network cards; `plugin` says what the machine has to put them
 * on, `before` is the VM being changed, whose cards keep their MAC
 * addresses.
 */
const props = defineProps<{ plugin: VmPlugin; before?: VirtualMachine; disabled?: boolean }>()
const form = defineModel<VmForm>('form', { required: true })
const emit = defineEmits<{ leave: [] }>()

const { t } = useI18n()
const openApp = useOpenApp()

const bridges = () => props.plugin.bridges

function setKind(slot: number, kind: NetworkKind): void {
  form.value = {
    ...form.value,
    nics: form.value.nics.map((nic) =>
      nic.slot === slot ? withNetworkKind(nic, kind, props.plugin) : nic,
    ),
  }
}

function label(kind: NetworkKind, device: string): string {
  return t(`vms.networks.on.${kind}`, { device })
}

function mac(slot: number): string | undefined {
  return props.before?.nics.find((nic) => nic.slot === slot)?.mac
}

function setModel(slot: number, model: VmNicModel): void {
  form.value = {
    ...form.value,
    nics: form.value.nics.map((nic) => (nic.slot === slot ? { ...nic, model } : nic)),
  }
}

function setDevice(slot: number, value: string): void {
  const device = value === '' ? null : value
  form.value = {
    ...form.value,
    nics: form.value.nics.map((nic) => (nic.slot === slot ? { ...nic, device } : nic)),
  }
}

/** Bridges are made on the control panel's Network page. */
function toNetwork(): void {
  emit('leave')
  openApp?.('settings', 'network')
}
</script>

<!-- Network cards: each with its model and its way to the network. -->
<template>
  <section class="grid gap-3">
    <p v-if="!form.nics.length" class="text-sm text-muted-foreground">
      {{ t('vms.devices.noNics') }}
    </p>
    <ul class="grid gap-2">
      <li
        v-for="nic in form.nics"
        :key="nic.slot"
        :data-nic="nic.slot"
        class="grid gap-3 rounded-lg border p-3 sm:grid-cols-[1fr_9rem_11rem_10rem_auto] sm:items-end"
      >
        <span class="grid gap-0.5 text-sm font-medium sm:self-center">
          <span class="flex items-center gap-2">
            <Network class="size-4 text-muted-foreground" />
            {{ t('vms.devices.nic', { n: nic.slot + 1 }) }}
          </span>
          <span v-if="mac(nic.slot)" class="font-mono text-xs font-normal text-muted-foreground">
            {{ t('vms.devices.mac', { mac: mac(nic.slot) }) }}
          </span>
        </span>
        <div class="grid gap-1.5">
          <Label :for="`vm-nic-${nic.slot}-model`" class="text-xs">{{
            t('vms.devices.model')
          }}</Label>
          <NativeSelect
            :id="`vm-nic-${nic.slot}-model`"
            :model-value="nic.model"
            :disabled="disabled"
            @update:model-value="(value) => setModel(nic.slot, value as VmNicModel)"
          >
            <option v-for="model in NIC_MODELS" :key="model" :value="model">
              {{ t(`vms.devices.models.${model}`) }}
            </option>
          </NativeSelect>
        </div>
        <div class="grid gap-1.5">
          <Label :for="`vm-nic-${nic.slot}-network`" class="text-xs">{{
            t('vms.devices.network')
          }}</Label>
          <NativeSelect
            :id="`vm-nic-${nic.slot}-network`"
            :model-value="nic.kind"
            :disabled="disabled"
            @update:model-value="(value) => setKind(nic.slot, value as NetworkKind)"
          >
            <option v-for="kind in NETWORK_KINDS" :key="kind" :value="kind">
              {{ t(`vms.networks.kinds.${kind}`) }}
            </option>
          </NativeSelect>
        </div>
        <div class="grid gap-1.5">
          <Label :for="`vm-nic-${nic.slot}-device`" class="text-xs">{{
            t(`vms.networks.device.${nic.kind}`)
          }}</Label>
          <NativeSelect
            v-if="nic.kind !== 'user'"
            :id="`vm-nic-${nic.slot}-device`"
            :model-value="nic.device ?? ''"
            :disabled="disabled || !networkChoices(plugin, nic.kind, nic.device).length"
            :aria-invalid="!nic.device"
            @update:model-value="(value) => setDevice(nic.slot, String(value))"
          >
            <option v-if="!nic.device" value="">{{ t('vms.networks.none') }}</option>
            <option
              v-for="device in networkChoices(plugin, nic.kind, nic.device)"
              :key="device"
              :value="device"
            >
              {{ label(nic.kind, device) }}
            </option>
          </NativeSelect>
          <span
            v-else
            :id="`vm-nic-${nic.slot}-device`"
            class="py-2 text-sm text-muted-foreground"
            >{{ t('vms.networks.throughNas') }}</span
          >
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          :aria-label="t('vms.devices.removeNic', { n: nic.slot + 1 })"
          :disabled="disabled"
          @click="form = removeNic(form, nic.slot)"
        >
          <Trash2 />
        </Button>
      </li>
    </ul>
    <Button
      type="button"
      size="sm"
      variant="outline"
      class="justify-self-start"
      :disabled="disabled || form.nics.length >= NICS_MAX"
      @click="form = addNic(form)"
    >
      <Plus />
      {{ t('vms.devices.addNic') }}
    </Button>
    <ul class="grid gap-1 text-xs text-muted-foreground">
      <li v-for="kind in NETWORK_KINDS" :key="kind">
        <span class="font-medium text-foreground">{{ t(`vms.networks.kinds.${kind}`) }}:</span>
        {{ t(`vms.networks.means.${kind}`) }}
      </li>
    </ul>
    <div v-if="!bridges().length" class="grid justify-items-start gap-2">
      <p class="text-xs text-muted-foreground">{{ t('vms.wizard.noBridge') }}</p>
      <Button v-if="openApp" type="button" size="sm" variant="outline" @click="toNetwork">
        {{ t('vms.wizard.toNetwork') }}
      </Button>
    </div>
  </section>
</template>
