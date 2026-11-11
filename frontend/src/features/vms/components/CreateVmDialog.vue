<script setup lang="ts">
import { Check, Disc3, Monitor, Network, Server } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VmFirmware, VmOs, VmPlugin } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { cn } from '@/shared/lib/utils'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { FolderPicker, useShareList } from '@/shared/ui/folder-picker'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { Switch } from '@/shared/ui/switch'

import { useVmActions } from '../api'
import {
  DISK_MAX_GIB,
  MEMORY_MIN_MIB,
  MEMORY_STEP_MIB,
  OS_PRESETS,
  STEP_PROBLEMS,
  formatMemory,
  isoPick,
  limits,
  presetForm,
  problems,
  request,
  type FormProblem,
  type VmForm,
  withPreset,
} from '../lib/vms'

/** Makes a VM in four steps; `taken`: the names of the VMs there are. */
const props = defineProps<{ plugin: VmPlugin; taken: string[] }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ created: [] }>()

const STEPS = ['system', 'resources', 'disk', 'review'] as const
const FIRMWARE: VmFirmware[] = ['uefi', 'bios']
const OS_ICONS: Record<VmOs, typeof Server> = { linux: Server, windows: Monitor, other: Disc3 }
/** A card that can be chosen: framed in the primary colour once it is. */
const CARD =
  'relative rounded-lg border transition-colors has-[:focus-visible]:ring-[3px] has-[:focus-visible]:ring-ring/50'
const CHOSEN = 'border-primary bg-primary/5 dark:bg-primary/10'

const { t } = useI18n()
const actions = useVmActions()
const jobs = useJobsStore()
const { data: shares } = useShareList()

const step = ref(0)
const form = ref<VmForm>(presetForm('linux', props.plugin))
const picking = ref(false)
const pending = ref(false)
const error = ref<string | null>(null)

watch(open, (isOpen) => {
  if (!isOpen) return
  form.value = presetForm('linux', props.plugin)
  step.value = 0
  error.value = null
})

const most = computed(() => limits(props.plugin))
const found = computed(() => problems(form.value, props.plugin, props.taken))
const has = (problem: FormProblem) => found.value.includes(problem)
const stepDone = computed(() => !STEP_PROBLEMS[step.value]!.some(has))

function choosePreset(os: VmOs): void {
  form.value = withPreset(form.value, os, props.plugin)
}

function pickIso(path: string): void {
  form.value = { ...form.value, iso: isoPick(path, shares.value) }
}

function next(): void {
  if (stepDone.value && step.value < STEPS.length - 1) step.value++
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

const reviewRows = computed(() => [
  [t('vms.wizard.name'), form.value.name.trim()],
  [t('vms.wizard.os'), t(`vms.os.${form.value.os}`)],
  ['CPU', t('vms.cpus', form.value.cpus)],
  [t('vms.memory'), formatMemory(form.value.memoryMib)],
  [t('vms.wizard.firmware'), t(`vms.wizard.${form.value.firmware}`)],
  [t('vms.disk'), `${form.value.diskGib} GiB`],
  [t('vms.wizard.iso'), form.value.iso?.shown ?? '–'],
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
                v-model="form.firmware"
                type="radio"
                name="vm-firmware"
                class="sr-only"
                :value="firmware"
              />
              <span class="font-semibold">{{ t(`vms.wizard.${firmware}`) }}</span>
              <span class="text-xs text-muted-foreground">{{
                t(`vms.wizard.${firmware}Hint`)
              }}</span>
            </label>
          </div>
        </fieldset>
      </template>

      <template v-else-if="step === 2">
        <div class="grid max-w-xs gap-2">
          <Label for="vm-disk">{{ t('vms.wizard.disk') }}</Label>
          <Input
            id="vm-disk"
            :model-value="form.diskGib"
            type="number"
            min="1"
            :max="DISK_MAX_GIB"
            :aria-invalid="has('disk')"
            @update:model-value="(value) => (form.diskGib = number(value))"
          />
          <p v-if="has('disk')" class="text-xs text-destructive">
            {{ t('vms.wizard.invalid.disk') }}
          </p>
          <p v-else class="text-xs text-muted-foreground">
            {{ t('vms.wizard.diskHint', { pool: plugin.storage?.pool ?? '–' }) }}
          </p>
        </div>
        <section class="grid gap-2" aria-labelledby="vm-iso-heading">
          <h4 id="vm-iso-heading" class="text-sm font-medium">{{ t('vms.wizard.iso') }}</h4>
          <div class="flex flex-wrap items-center gap-2 rounded-lg border p-3 text-sm">
            <Disc3 class="size-4 shrink-0 text-muted-foreground" />
            <span
              v-if="form.iso"
              class="min-w-0 flex-1 truncate font-mono text-xs"
              data-testid="vm-iso"
            >
              {{ form.iso.shown }}
            </span>
            <span v-else class="min-w-0 flex-1 text-muted-foreground">{{
              t('vms.wizard.noIso')
            }}</span>
            <Button type="button" size="sm" variant="outline" @click="picking = true">
              {{ form.iso ? t('vms.wizard.changeIso') : t('vms.wizard.pickIso') }}
            </Button>
            <Button
              v-if="form.iso"
              type="button"
              size="sm"
              variant="ghost"
              @click="form = { ...form, iso: null }"
            >
              {{ t('vms.wizard.removeIso') }}
            </Button>
          </div>
          <p class="text-xs text-muted-foreground">{{ t('vms.wizard.isoHint') }}</p>
        </section>
      </template>

      <template v-else>
        <fieldset class="grid gap-2">
          <legend class="mb-2 text-sm font-medium">{{ t('vms.wizard.network') }}</legend>
          <div role="radiogroup" class="grid gap-2 sm:grid-cols-2">
            <label
              v-for="choice in [null, ...plugin.bridges]"
              :key="choice ?? 'user'"
              :data-network="choice ?? 'user'"
              :class="
                cn(
                  CARD,
                  'grid cursor-pointer gap-1 p-3 text-sm',
                  form.bridge === choice ? CHOSEN : 'hover:bg-accent/60',
                )
              "
            >
              <input
                v-model="form.bridge"
                type="radio"
                name="vm-network"
                class="sr-only"
                :value="choice"
              />
              <span class="flex items-center gap-2 font-semibold">
                <Network class="size-4 text-muted-foreground" />
                {{ choice ? t('vms.wizard.bridge', { bridge: choice }) : t('vms.wizard.user') }}
              </span>
              <span class="text-xs text-muted-foreground">{{
                choice ? t('vms.wizard.bridgeHint') : t('vms.wizard.userHint')
              }}</span>
            </label>
          </div>
          <p v-if="!plugin.bridges.length" class="text-xs text-muted-foreground">
            {{ t('vms.wizard.noBridge') }}
          </p>
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

    <FolderPicker
      v-model:open="picking"
      :value="form.iso?.shown ?? ''"
      files=".iso"
      @pick="pickIso"
    />

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
