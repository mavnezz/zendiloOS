<script setup lang="ts">
import { HardDrive, Plus, Trash2 } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmDiskBus } from '@/shared/api/types'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'

import {
  DISK_BUSES,
  DISK_MAX_GIB,
  DISKS_MAX,
  addDisk,
  minimumGib,
  removeDisk,
  type VmForm,
} from '../lib/vms'

/** A VM's disks; `before`: the VM being changed, whose disks only grow. */
const props = defineProps<{ pool?: string; before?: VirtualMachine; disabled?: boolean }>()
const form = defineModel<VmForm>('form', { required: true })

const { t } = useI18n()

function number(value: string | number): number {
  return typeof value === 'number' ? value : Number(value.trim() === '' ? NaN : value)
}

function setSize(slot: number, value: string | number): void {
  form.value = {
    ...form.value,
    disks: form.value.disks.map((disk) =>
      disk.slot === slot ? { ...disk, sizeGib: number(value) } : disk,
    ),
  }
}

function setBus(slot: number, bus: VmDiskBus): void {
  form.value = {
    ...form.value,
    disks: form.value.disks.map((disk) => (disk.slot === slot ? { ...disk, bus } : disk)),
  }
}

const invalid = (size: number, slot: number) =>
  !Number.isInteger(size) ||
  size < Math.max(1, minimumGib(props.before, slot)) ||
  size > DISK_MAX_GIB
</script>

<!-- Disks: each a sparse zvol in its slot, sized and attached; more added, some removed. -->
<template>
  <section class="grid gap-3" aria-labelledby="vm-disks-heading">
    <h4 id="vm-disks-heading" class="sr-only">{{ t('vms.edit.tabs.disks') }}</h4>
    <ul class="grid gap-2">
      <li
        v-for="disk in form.disks"
        :key="disk.slot"
        :data-disk="disk.slot"
        class="grid gap-3 rounded-lg border p-3 sm:grid-cols-[1fr_8rem_8rem_auto] sm:items-end"
      >
        <span class="flex items-center gap-2 text-sm font-medium sm:self-center">
          <HardDrive class="size-4 text-muted-foreground" />
          {{ t('vms.devices.disk', { n: disk.slot + 1 }) }}
        </span>
        <div class="grid gap-1.5">
          <Label :for="`vm-disk-${disk.slot}-size`" class="text-xs">{{
            t('vms.devices.size')
          }}</Label>
          <Input
            :id="`vm-disk-${disk.slot}-size`"
            :model-value="disk.sizeGib"
            type="number"
            :min="minimumGib(before, disk.slot)"
            :max="DISK_MAX_GIB"
            :disabled="disabled"
            :aria-invalid="invalid(disk.sizeGib, disk.slot)"
            @update:model-value="(value) => setSize(disk.slot, value)"
          />
        </div>
        <div class="grid gap-1.5">
          <Label :for="`vm-disk-${disk.slot}-bus`" class="text-xs">{{
            t('vms.devices.bus')
          }}</Label>
          <NativeSelect
            :id="`vm-disk-${disk.slot}-bus`"
            :model-value="disk.bus"
            :disabled="disabled"
            @update:model-value="(value) => setBus(disk.slot, value as VmDiskBus)"
          >
            <option v-for="bus in DISK_BUSES" :key="bus" :value="bus">
              {{ t(`vms.devices.buses.${bus}`) }}
            </option>
          </NativeSelect>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          :aria-label="t('vms.devices.removeDisk', { n: disk.slot + 1 })"
          :disabled="disabled || form.disks.length <= 1"
          @click="form = removeDisk(form, disk.slot)"
        >
          <Trash2 />
        </Button>
        <p
          v-if="before && minimumGib(before, disk.slot) > 1"
          class="text-xs text-muted-foreground sm:col-span-4"
          :class="invalid(disk.sizeGib, disk.slot) && 'text-destructive'"
        >
          {{ t('vms.devices.growOnly', { min: minimumGib(before, disk.slot) }) }}
        </p>
      </li>
    </ul>
    <div class="flex flex-wrap items-center gap-3">
      <Button
        type="button"
        size="sm"
        variant="outline"
        :disabled="disabled || form.disks.length >= DISKS_MAX"
        @click="form = addDisk(form)"
      >
        <Plus />
        {{ t('vms.devices.addDisk') }}
      </Button>
      <p v-if="pool" class="text-xs text-muted-foreground">
        {{ t('vms.devices.sizeHint', { pool }) }}
      </p>
    </div>
  </section>
</template>
