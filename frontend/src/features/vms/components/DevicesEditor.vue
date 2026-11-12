<script setup lang="ts">
import { Cpu, LoaderCircle, TriangleAlert, Usb } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmHostNeeds, VmHostPci } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'

import { useVmDevices } from '../api'
import {
  PCI_MAX,
  USB_MAX,
  groupChosen,
  iommuHint,
  pciGroups,
  pciKind,
  pciLabel,
  usbChoices,
  withGroup,
  withUsb,
  type PciGroup,
} from '../lib/devices'
import type { VmForm } from '../lib/vms'

/**
 * The host's devices a VM is given: USB devices, which change while it
 * runs, and PCI devices by IOMMU group, which change only while it is off
 * (`fixed`). `before` is the VM being changed.
 */
const props = defineProps<{ before?: VirtualMachine; fixed?: boolean }>()
const form = defineModel<VmForm>('form', { required: true })

const { t } = useI18n()
const { data: devices, error, isPending } = useVmDevices()

const usb = computed(() => usbChoices(devices.value?.usb ?? [], form.value.usb))
const groups = computed(() => pciGroups(devices.value?.pci ?? []))
const iommu = computed(() => devices.value?.iommu)
/** PCI devices the VM has that the host does not show any more. */
const gonePci = computed(() =>
  form.value.pci.filter(
    (address) => !groups.value.some((group) => group.members.some((d) => d.address === address)),
  ),
)
const pciFixed = computed(() => props.fixed || !iommu.value?.on)

function setUsb(id: string, on: boolean): void {
  form.value = { ...form.value, usb: withUsb(form.value.usb, id, on) }
}

function setGroup(group: PciGroup, on: boolean): void {
  form.value = { ...form.value, pci: withGroup(form.value.pci, group, on) }
}

function removePci(address: string): void {
  form.value = { ...form.value, pci: form.value.pci.filter((other) => other !== address) }
}

function needsText(needs: VmHostNeeds): string {
  switch (needs.kind) {
    case 'disk':
      return t('vms.hostDevices.needs.disk', { disk: needs.disk })
    case 'console':
      return t('vms.hostDevices.needs.console')
    case 'network':
      return needs.interface
        ? t('vms.hostDevices.needs.network', { interface: needs.interface })
        : t('vms.hostDevices.needs.networkUnknown')
    default:
      return t('vms.hostDevices.needs.bridge')
  }
}

/** Why a group stays with the host, if it does. */
function blockedText(group: PciGroup): string | null {
  const device = group.blocked
  if (!device) return null
  if (group.id === null) return t('vms.hostDevices.needs.noGroup')
  return `${t('vms.hostDevices.heldBy', { device: device.address })}: ${device.needed ? needsText(device.needed) : ''}`
}

function usbTaken(id: string): boolean {
  return form.value.usb.length >= USB_MAX && !form.value.usb.includes(id)
}

function groupTaken(group: PciGroup): boolean {
  const adding = group.members.filter((d) => !form.value.pci.includes(d.address)).length
  return form.value.pci.length + adding > PCI_MAX
}

const kind = (device: VmHostPci) => t(`vms.hostDevices.kinds.${pciKind(device.class)}`)
</script>

<!-- The NAS's USB and PCI devices a VM is given. -->
<template>
  <section class="grid gap-5" data-testid="vm-host-devices">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <div v-if="isPending" class="flex items-center gap-2 text-sm text-muted-foreground">
      <LoaderCircle class="size-4 animate-spin" />
    </div>

    <fieldset class="grid gap-2">
      <legend class="mb-1 flex items-center gap-2 text-sm font-medium">
        <Usb class="size-4 text-muted-foreground" />
        {{ t('vms.hostDevices.usb') }}
      </legend>
      <p class="text-xs text-muted-foreground">{{ t('vms.hostDevices.usbHint') }}</p>
      <p v-if="!usb.length && !isPending" class="text-sm text-muted-foreground">
        {{ t('vms.hostDevices.usbNone') }}
      </p>
      <ul class="grid gap-2">
        <li v-for="choice in usb" :key="choice.id" :data-usb="choice.id">
          <label class="flex items-start gap-3 rounded-lg border p-3 text-sm">
            <input
              type="checkbox"
              class="mt-0.5 size-4 accent-primary"
              :checked="form.usb.includes(choice.id)"
              :disabled="usbTaken(choice.id)"
              @change="setUsb(choice.id, ($event.target as HTMLInputElement).checked)"
            />
            <span class="grid min-w-0 flex-1 gap-0.5">
              <span class="flex flex-wrap items-center gap-2 font-medium">
                {{ choice.label }}
                <span class="font-mono text-xs text-muted-foreground">{{ choice.id }}</span>
              </span>
              <span v-if="choice.port" class="text-xs text-muted-foreground">{{
                t('vms.hostDevices.port', { port: choice.port })
              }}</span>
              <span v-else class="text-xs text-amber-700 dark:text-amber-300">{{
                t('vms.hostDevices.notPlugged')
              }}</span>
              <span v-if="choice.twins" class="text-xs text-amber-700 dark:text-amber-300">{{
                t('vms.hostDevices.twins')
              }}</span>
            </span>
          </label>
        </li>
      </ul>
    </fieldset>

    <fieldset class="grid gap-2" data-testid="vm-pci">
      <legend class="mb-1 flex items-center gap-2 text-sm font-medium">
        <Cpu class="size-4 text-muted-foreground" />
        {{ t('vms.hostDevices.pci') }}
      </legend>
      <p class="text-xs text-muted-foreground">{{ t('vms.hostDevices.pciHint') }}</p>
      <Alert v-if="iommu && !iommu.on" data-testid="vm-iommu-off">
        <TriangleAlert />
        <div class="grid gap-1">
          <p>{{ t('vms.hostDevices.iommuOff') }}</p>
          <p class="text-xs">{{ t(`vms.hostDevices.iommuHint.${iommuHint(iommu)}`) }}</p>
          <p v-if="iommu.cmdline.length" class="font-mono text-xs">
            {{ t('vms.hostDevices.cmdline', { flags: iommu.cmdline.join(' ') }) }}
          </p>
        </div>
      </Alert>
      <template v-else-if="iommu">
        <Alert
          class="border-amber-500/40 bg-amber-500/5 text-amber-800 dark:text-amber-300"
          data-testid="vm-pci-warning"
        >
          <TriangleAlert />
          {{ t('vms.hostDevices.pciWarning') }}
        </Alert>
        <p v-if="fixed" class="text-xs text-muted-foreground">
          {{ t('vms.hostDevices.pciRunning') }}
        </p>
        <p v-if="!groups.length" class="text-sm text-muted-foreground">
          {{ t('vms.hostDevices.pciNone') }}
        </p>
        <ul class="grid gap-2">
          <li
            v-for="group in groups"
            :key="group.members[0]?.address"
            :data-group="group.id ?? 'none'"
          >
            <label
              class="flex items-start gap-3 rounded-lg border p-3 text-sm"
              :class="group.blocked ? 'opacity-70' : ''"
            >
              <input
                type="checkbox"
                class="mt-0.5 size-4 accent-primary"
                :checked="groupChosen(form.pci, group) !== 'none'"
                :indeterminate="groupChosen(form.pci, group) === 'some'"
                :disabled="
                  pciFixed ||
                  (group.blocked !== null && groupChosen(form.pci, group) === 'none') ||
                  (groupTaken(group) && groupChosen(form.pci, group) === 'none')
                "
                @change="setGroup(group, ($event.target as HTMLInputElement).checked)"
              />
              <span class="grid min-w-0 flex-1 gap-1">
                <span class="text-xs text-muted-foreground">{{
                  group.id === null ? '–' : t('vms.hostDevices.group', { n: group.id })
                }}</span>
                <span
                  v-for="device in group.members"
                  :key="device.address"
                  class="flex flex-wrap items-center gap-2"
                >
                  <Badge variant="outline">{{ kind(device) }}</Badge>
                  <span class="font-medium">{{ pciLabel(device) }}</span>
                  <span class="font-mono text-xs text-muted-foreground">{{ device.address }}</span>
                  <span v-if="device.driver" class="text-xs text-muted-foreground">{{
                    t('vms.hostDevices.driver', { driver: device.driver })
                  }}</span>
                </span>
                <span
                  v-if="blockedText(group)"
                  class="text-xs text-muted-foreground"
                  data-testid="vm-pci-blocked"
                  >{{ blockedText(group) }}</span
                >
              </span>
            </label>
          </li>
        </ul>
      </template>
      <ul v-if="gonePci.length" class="grid gap-2">
        <li
          v-for="address in gonePci"
          :key="address"
          class="flex items-center gap-3 rounded-lg border border-dashed p-3 text-sm"
          :data-pci-gone="address"
        >
          <span class="font-mono">{{ address }}</span>
          <span class="flex-1 text-xs text-muted-foreground">{{ t('vms.hostDevices.gone') }}</span>
          <button
            type="button"
            class="text-xs text-destructive underline-offset-2 hover:underline disabled:opacity-50"
            :disabled="fixed"
            @click="removePci(address)"
          >
            {{ t('vms.delete') }}
          </button>
        </li>
      </ul>
    </fieldset>
  </section>
</template>
