<script setup lang="ts">
import { Check, Disc3, Monitor, Server } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VmFirmware, VmImage, VmOs, VmPlugin } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { cn } from '@/shared/lib/utils'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { Switch } from '@/shared/ui/switch'

import { useVmActions } from '../api'
import {
  MEMORY_MIN_MIB,
  MEMORY_STEP_MIB,
  OS_PRESETS,
  STEP_PROBLEMS,
  formatMemory,
  limits,
  presetForm,
  problems,
  request,
  type FormProblem,
  type VmForm,
  withFirmware,
  withPreset,
} from '../lib/vms'
import DevicesEditor from './DevicesEditor.vue'
import DisksEditor from './DisksEditor.vue'
import MediaEditor from './MediaEditor.vue'
import NicsEditor from './NicsEditor.vue'
import SecurityEditor from './SecurityEditor.vue'

/**
 * Makes a VM in four steps; `taken`: the names of the VMs there are,
 * `images`: the library.
 */
const props = defineProps<{ plugin: VmPlugin; taken: string[]; images: VmImage[] }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ created: []; library: [] }>()

const STEPS = ['system', 'resources', 'storage', 'review'] as const
const FIRMWARE: VmFirmware[] = ['uefi', 'bios']
const OS_ICONS: Record<VmOs, typeof Server> = { linux: Server, windows: Monitor, other: Disc3 }
/** A card that can be chosen: framed in the primary colour once it is. */
const CARD =
  'relative rounded-lg border transition-colors has-[:focus-visible]:ring-[3px] has-[:focus-visible]:ring-ring/50'
const CHOSEN = 'border-primary bg-primary/5 dark:bg-primary/10'

const { t } = useI18n()
const actions = useVmActions()
const jobs = useJobsStore()

const step = ref(0)
const form = ref<VmForm>(presetForm('linux', props.plugin))
const pending = ref(false)
const error = ref<string | null>(null)

watch(open, (isOpen) => {
  if (!isOpen) return
  form.value = presetForm('linux', props.plugin)
  step.value = 0
  error.value = null
})

const most = computed(() => limits(props.plugin))
const found = computed(() => problems(form.value, props.plugin, props.taken, props.images))
const has = (problem: FormProblem) => found.value.includes(problem)
const stepDone = computed(() => !STEP_PROBLEMS[step.value]!.some(has))

function choosePreset(os: VmOs): void {
  form.value = withPreset(form.value, os, props.plugin)
}

function next(): void {
  if (stepDone.value && step.value < STEPS.length - 1) step.value++
}

function toLibrary(): void {
  open.value = false
  emit('library')
}

async function create(): Promise<void> {
  if (found.value.length) return
  pending.value = true
  error.value = null
  try {
    const made = await actions.create(request(form.value))
    jobs.update(made.job)
    emit('created')
    open.value = false
  } catch (problem) {
    error.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}

/** Numbers typed into a field: what can be read of them. */
function number(value: string | number): number {
  return typeof value === 'number' ? value : Number(value.trim() === '' ? NaN : value)
}

const imageName = (id: string | null) =>
  id ? (props.images.find((image) => image.id === id)?.name ?? id) : t('vms.devices.noImage')

const reviewRows = computed(() => [
  [t('vms.wizard.name'), form.value.name.trim()],
  [t('vms.wizard.os'), t(`vms.os.${form.value.os}`)],
  ['CPU', t('vms.cpus', form.value.cpus)],
  [t('vms.memory'), formatMemory(form.value.memoryMib)],
  [t('vms.wizard.firmware'), t(`vms.wizard.${form.value.firmware}`)],
  [
    t('vms.disks', form.value.disks.length),
    form.value.disks.map((disk) => `${disk.sizeGib} GiB`).join(', '),
  ],
  [
    t('vms.wizard.media'),
    form.value.drives.map((drive) => imageName(drive.image)).join(', ') || '–',
  ],
  [
    t('vms.wizard.security'),
    [
      form.value.tpm ? t('vms.security.tpm') : null,
      form.value.secureBoot ? t('vms.security.secureBoot') : null,
    ]
      .filter(Boolean)
      .join(', ') || t('vms.wizard.none'),
  ],
  [
    t('vms.wizard.hostDevices'),
    form.value.usb.length || form.value.pci.length
      ? t('vms.hostDevices.summary', { usb: form.value.usb.length, pci: form.value.pci.length })
      : t('vms.wizard.none'),
  ],
])
</script>

<template>
  <StepDialog
    v-model:open="open"
    :title="t('vms.wizard.title')"
    :close-label="t('desktop.close')"
    :steps="STEPS.map((name) => t(`vms.wizard.steps.${name}`))"
    :step="step"
    wide
  >
    <h3 class="sr-only">{{ t(`vms.wizard.steps.${STEPS[step]}`) }}</h3>
    <form id="vm-wizard" class="grid gap-6" @submit.prevent="step < 3 ? next() : create()">
      <template v-if="step === 0">
        <div class="grid max-w-md gap-2">
          <Label for="vm-name">{{ t('vms.wizard.name') }}</Label>
          <Input
            id="vm-name"
            v-model="form.name"
            :placeholder="t('vms.wizard.namePlaceholder')"
            maxlength="48"
            autocomplete="off"
            :aria-invalid="form.name !== '' && (has('name') || has('nameTaken'))"
          />
          <p v-if="form.name !== '' && has('name')" class="text-xs text-destructive">
            {{ t('vms.wizard.invalid.name') }}
          </p>
          <p v-if="has('nameTaken')" class="text-xs text-destructive">
            {{ t('vms.wizard.invalid.nameTaken') }}
          </p>
        </div>
        <fieldset class="grid gap-2">
          <legend class="mb-2 text-sm font-medium">{{ t('vms.wizard.os') }}</legend>
          <div role="radiogroup" class="grid gap-2 sm:grid-cols-3">
            <label
              v-for="os in OS_PRESETS"
              :key="os"
              :data-os="os"
              :class="
                cn(
                  CARD,
                  'grid cursor-pointer gap-2 p-3 text-sm',
                  form.os === os ? CHOSEN : 'hover:bg-accent/60',
                )
              "
            >
              <input
                type="radio"
                name="vm-os"
                class="sr-only"
                :value="os"
                :checked="form.os === os"
                @change="choosePreset(os)"
              />
              <span
                v-if="form.os === os"
                class="absolute -top-px -left-px grid size-4 place-items-center rounded-tl-lg rounded-br-md bg-primary text-primary-foreground"
                aria-hidden="true"
              >
                <Check class="size-3" :stroke-width="3" />
              </span>
              <span class="flex items-center gap-2 font-semibold">
                <component :is="OS_ICONS[os]" class="size-4 text-muted-foreground" />
                {{ t(`vms.os.${os}`) }}
              </span>
              <span class="text-xs text-muted-foreground">{{ t(`vms.osHints.${os}`) }}</span>
            </label>
          </div>
        </fieldset>
      </template>

      <template v-else-if="step === 1">
        <div class="grid gap-4 sm:grid-cols-2">
          <div class="grid content-start gap-2">
            <Label for="vm-cpus">{{ t('vms.wizard.cpus', { n: plugin.cores }) }}</Label>
            <Input
              id="vm-cpus"
              :model-value="form.cpus"
              type="number"
              min="1"
              :max="most.cpus"
              :aria-invalid="has('cpus')"
              @update:model-value="(value) => (form.cpus = number(value))"
            />
            <p v-if="has('cpus')" class="text-xs text-destructive">
              {{ t('vms.wizard.invalid.cpus', { max: most.cpus }) }}
            </p>
          </div>
          <div class="grid content-start gap-2">
            <Label for="vm-memory">{{
              t('vms.wizard.memory', { total: formatBytes(plugin.memory_bytes) })
            }}</Label>
            <Input
              id="vm-memory"
              :model-value="form.memoryMib"
              type="number"
              :min="MEMORY_MIN_MIB"
              :max="most.memoryMib"
              :step="MEMORY_STEP_MIB"
              :aria-invalid="has('memory')"
              @update:model-value="(value) => (form.memoryMib = number(value))"
            />
            <p v-if="has('memory')" class="text-xs text-destructive">
              {{ t('vms.wizard.invalid.memory', { max: most.memoryMib }) }}
            </p>
            <p v-else class="text-xs text-muted-foreground">{{ t('vms.wizard.memoryHint') }}</p>
          </div>
        </div>
        <fieldset class="grid gap-2">
          <legend class="mb-2 text-sm font-medium">{{ t('vms.wizard.firmware') }}</legend>
          <div role="radiogroup" class="grid gap-2 sm:grid-cols-2">
            <label
              v-for="firmware in FIRMWARE"
              :key="firmware"
              :data-firmware="firmware"
              :class="
                cn(
                  CARD,
                  'grid cursor-pointer gap-1 p-3 text-sm',
                  form.firmware === firmware ? CHOSEN : 'hover:bg-accent/60',
                )
              "
            >
              <input
                type="radio"
                name="vm-firmware"
                class="sr-only"
                :value="firmware"
                :checked="form.firmware === firmware"
                @change="form = withFirmware(form, firmware)"
              />
              <span class="font-semibold">{{ t(`vms.wizard.${firmware}`) }}</span>
              <span class="text-xs text-muted-foreground">{{
                t(`vms.wizard.${firmware}Hint`)
              }}</span>
            </label>
          </div>
        </fieldset>
        <SecurityEditor v-model:form="form" :plugin="plugin" />
      </template>

      <template v-else-if="step === 2">
        <DisksEditor v-model:form="form" :locations="plugin.locations" />
        <p v-if="has('disk')" class="text-xs text-destructive">
          {{ t('vms.wizard.invalid.disk') }}
        </p>
        <MediaEditor v-model:form="form" :images="images" @library="toLibrary" />
        <p v-if="has('boot')" class="text-xs text-destructive">
          {{ t('vms.wizard.invalid.boot') }}
        </p>
      </template>

      <template v-else>
        <fieldset class="grid gap-2">
          <legend class="mb-2 text-sm font-medium">{{ t('vms.wizard.network') }}</legend>
          <NicsEditor v-model:form="form" :plugin="plugin" @leave="open = false" />
          <p v-if="has('network')" class="text-xs text-destructive">
            {{ t('vms.wizard.invalid.network') }}
          </p>
        </fieldset>
        <fieldset class="grid gap-2">
          <legend class="mb-2 text-sm font-medium">{{ t('vms.wizard.hostDevices') }}</legend>
          <DevicesEditor v-model:form="form" />
        </fieldset>
        <div class="flex items-center gap-3">
          <Switch id="vm-autostart" v-model="form.autostart" />
          <Label for="vm-autostart" class="font-normal">{{ t('vms.wizard.autostart') }}</Label>
        </div>
        <section class="grid gap-2" aria-labelledby="vm-review-heading">
          <h4 id="vm-review-heading" class="text-sm font-medium">{{ t('vms.wizard.review') }}</h4>
          <dl class="grid gap-x-6 gap-y-1 rounded-lg border p-3 text-sm sm:grid-cols-[auto_1fr]">
            <template v-for="[label, value] in reviewRows" :key="label">
              <dt class="text-muted-foreground">{{ label }}</dt>
              <dd class="min-w-0 truncate font-medium">{{ value }}</dd>
            </template>
          </dl>
        </section>
        <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      </template>
    </form>

    <template #footer>
      <Button v-if="step > 0" type="button" variant="outline" class="mr-auto" @click="step--">
        {{ t('vms.wizard.back') }}
      </Button>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button v-if="step < 3" type="submit" form="vm-wizard" :disabled="!stepDone">
        {{ t('vms.wizard.next') }}
      </Button>
      <Button v-else type="submit" form="vm-wizard" :disabled="pending || found.length > 0">
        {{ pending ? t('vms.wizard.creating') : t('vms.wizard.submit') }}
      </Button>
    </template>
  </StepDialog>
</template>
