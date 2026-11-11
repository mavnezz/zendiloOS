<script setup lang="ts">
import { Check, Info, TriangleAlert } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmFirmware, VmImage, VmPlugin } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { PageTabs } from '@/shared/ui/page'
import { Switch } from '@/shared/ui/switch'

import { useVmActions } from '../api'
import {
  MEMORY_MIN_MIB,
  MEMORY_STEP_MIB,
  fromVm,
  hardwareChanged,
  limits,
  problems,
  removedDisks,
  updateRequest,
  type FormProblem,
  type VmForm,
} from '../lib/vms'
import DisksEditor from './DisksEditor.vue'
import MediaEditor from './MediaEditor.vue'
import NicsEditor from './NicsEditor.vue'

/**
 * Changes a VM. While it is not shut off, only its name, autostart and the
 * images in its drives change; the rest waits until it is off.
 * `taken`: the other VMs' names.
 */
const props = defineProps<{
  vm: VirtualMachine
  plugin: VmPlugin
  taken: string[]
  images: VmImage[]
}>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ library: [] }>()

const TABS = ['general', 'hardware', 'disks', 'network', 'media'] as const
const FIRMWARE: VmFirmware[] = ['uefi', 'bios']
const CARD =
  'relative rounded-lg border transition-colors has-[:focus-visible]:ring-[3px] has-[:focus-visible]:ring-ring/50'
const CHOSEN = 'border-primary bg-primary/5 dark:bg-primary/10'

const { t } = useI18n()
const actions = useVmActions()

const tab = ref<string>('general')
const form = ref<VmForm>(fromVm(props.vm))
const confirm = ref('')
const pending = ref(false)
const error = ref<string | null>(null)

watch(
  open,
  (isOpen) => {
    if (!isOpen) return
    form.value = fromVm(props.vm)
    tab.value = 'general'
    confirm.value = ''
    error.value = null
  },
  { immediate: true },
)

/** Not shut off: its hardware stays as it is. */
const fixed = computed(() => props.vm.state !== 'shut_off')
const most = computed(() => limits(props.plugin))
const found = computed(() =>
  problems(form.value, props.plugin, props.taken, props.images, props.vm),
)
const has = (problem: FormProblem) => found.value.includes(problem)
const hardware = computed(() => hardwareChanged(form.value, props.vm))
const removed = computed(() => removedDisks(form.value, props.vm))
const confirmed = computed(() => !removed.value.length || confirm.value.trim() === props.vm.name)
const canSave = computed(
  () =>
    !found.value.length && !(fixed.value && hardware.value) && confirmed.value && !pending.value,
)

function number(value: string | number): number {
  return typeof value === 'number' ? value : Number(value.trim() === '' ? NaN : value)
}

function toLibrary(): void {
  open.value = false
  emit('library')
}

async function save(): Promise<void> {
  if (!canSave.value) return
  pending.value = true
  error.value = null
  try {
    const request = updateRequest(form.value, removed.value.length ? confirm.value : undefined)
    await actions.update(props.vm.id, request)
    open.value = false
  } catch (problem) {
    error.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}

const tabs = computed(() => TABS.map((id) => ({ id, label: t(`vms.edit.tabs.${id}`) })))
</script>

<!-- A VM changed: its name, resources, disks, cards, media and boot order. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('vms.edit.title', { name: vm.name })"
    :close-label="t('desktop.close')"
    wide
  >
    <PageTabs v-model="tab" :tabs="tabs" :label="t('vms.edit.title', { name: vm.name })" />
    <form id="vm-edit" class="grid gap-5" @submit.prevent="save">
      <Alert v-if="fixed" data-testid="vm-edit-running">
        <Info />
        {{ t('vms.edit.running') }}
      </Alert>

      <template v-if="tab === 'general'">
        <div class="grid max-w-md gap-2">
          <Label for="vm-edit-name">{{ t('vms.wizard.name') }}</Label>
          <Input
            id="vm-edit-name"
            v-model="form.name"
            maxlength="48"
            autocomplete="off"
            :aria-invalid="has('name') || has('nameTaken')"
          />
          <p v-if="has('name')" class="text-xs text-destructive">
            {{ t('vms.wizard.invalid.name') }}
          </p>
          <p v-if="has('nameTaken')" class="text-xs text-destructive">
            {{ t('vms.wizard.invalid.nameTaken') }}
          </p>
        </div>
        <div class="flex items-center gap-3">
          <Switch id="vm-edit-autostart" v-model="form.autostart" />
          <Label for="vm-edit-autostart" class="font-normal">{{ t('vms.wizard.autostart') }}</Label>
        </div>
      </template>

      <template v-else-if="tab === 'hardware'">
        <div class="grid gap-4 sm:grid-cols-2">
          <div class="grid content-start gap-2">
            <Label for="vm-edit-cpus">{{ t('vms.wizard.cpus', { n: plugin.cores }) }}</Label>
            <Input
              id="vm-edit-cpus"
              :model-value="form.cpus"
              type="number"
              min="1"
              :max="most.cpus"
              :disabled="fixed"
              :aria-invalid="has('cpus')"
              @update:model-value="(value) => (form.cpus = number(value))"
            />
            <p v-if="has('cpus')" class="text-xs text-destructive">
              {{ t('vms.wizard.invalid.cpus', { max: most.cpus }) }}
            </p>
          </div>
          <div class="grid content-start gap-2">
            <Label for="vm-edit-memory">{{
              t('vms.wizard.memory', { total: formatBytes(plugin.memory_bytes) })
            }}</Label>
            <Input
              id="vm-edit-memory"
              :model-value="form.memoryMib"
              type="number"
              :min="MEMORY_MIN_MIB"
              :max="most.memoryMib"
              :step="MEMORY_STEP_MIB"
              :disabled="fixed"
              :aria-invalid="has('memory')"
              @update:model-value="(value) => (form.memoryMib = number(value))"
            />
            <p v-if="has('memory')" class="text-xs text-destructive">
              {{ t('vms.wizard.invalid.memory', { max: most.memoryMib }) }}
            </p>
          </div>
        </div>
        <fieldset class="grid gap-2" :disabled="fixed">
          <legend class="mb-2 text-sm font-medium">{{ t('vms.wizard.firmware') }}</legend>
          <div role="radiogroup" class="grid gap-2 sm:grid-cols-2">
            <label
              v-for="firmware in FIRMWARE"
              :key="firmware"
              :data-firmware="firmware"
              :class="
                cn(
                  CARD,
                  'grid gap-1 p-3 text-sm',
                  fixed ? 'opacity-60' : 'cursor-pointer',
                  form.firmware === firmware ? CHOSEN : 'hover:bg-accent/60',
                )
              "
            >
              <input
                v-model="form.firmware"
                type="radio"
                name="vm-edit-firmware"
                class="sr-only"
                :value="firmware"
              />
              <span
                v-if="form.firmware === firmware"
                class="absolute -top-px -left-px grid size-4 place-items-center rounded-tl-lg rounded-br-md bg-primary text-primary-foreground"
                aria-hidden="true"
              >
                <Check class="size-3" :stroke-width="3" />
              </span>
              <span class="font-semibold">{{ t(`vms.wizard.${firmware}`) }}</span>
              <span class="text-xs text-muted-foreground">{{
                t(`vms.wizard.${firmware}Hint`)
              }}</span>
            </label>
          </div>
        </fieldset>
        <Alert
          v-if="form.firmware !== vm.firmware"
          class="border-amber-500/40 bg-amber-500/5 text-amber-800 dark:text-amber-300"
          data-testid="vm-firmware-warning"
        >
          <TriangleAlert />
          {{ t('vms.edit.firmwareWarning') }}
        </Alert>
      </template>

      <template v-else-if="tab === 'disks'">
        <DisksEditor v-model:form="form" :pool="vm.pool" :before="vm" :disabled="fixed" />
        <p v-if="has('disk')" class="text-xs text-destructive">
          {{ t('vms.wizard.invalid.disk') }}
        </p>
      </template>

      <NicsEditor
        v-else-if="tab === 'network'"
        v-model:form="form"
        :bridges="plugin.bridges"
        :before="vm"
        :disabled="fixed"
        @leave="open = false"
      />

      <template v-else>
        <MediaEditor v-model:form="form" :images="images" :fixed="fixed" @library="toLibrary" />
        <p v-if="has('boot')" class="text-xs text-destructive">
          {{ t('vms.wizard.invalid.boot') }}
        </p>
        <p v-if="has('image')" class="text-xs text-destructive">
          {{ t('vms.wizard.invalid.image') }}
        </p>
      </template>

      <div
        v-if="removed.length"
        class="grid gap-2 rounded-lg border border-destructive/40 bg-destructive/5 p-3"
        data-testid="vm-edit-removing"
      >
        <p class="text-sm text-destructive">
          {{
            t(
              'vms.edit.removing',
              { disks: removed.map((slot) => slot + 1).join(', ') },
              removed.length,
            )
          }}
        </p>
        <Label for="vm-edit-confirm">{{ t('vms.edit.confirm', { name: vm.name }) }}</Label>
        <Input
          id="vm-edit-confirm"
          v-model="confirm"
          class="max-w-xs"
          autocomplete="off"
          spellcheck="false"
        />
      </div>
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
    </form>

    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button type="submit" form="vm-edit" :disabled="!canSave">
        {{ pending ? t('vms.edit.saving') : t('vms.edit.save') }}
      </Button>
    </template>
  </ModalDialog>
</template>
