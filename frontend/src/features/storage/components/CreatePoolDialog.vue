<script setup lang="ts">
import { Check, HardDrive, MemoryStick } from 'lucide-vue-next'
import { computed, nextTick, ref, useTemplateRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Disk, NewPoolLayout, PoolPlan } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { useJobsStore } from '@/shared/jobs/store'

import { createPool, planPool, useDisks, useEnclosure } from '../api'
import { usageLabel } from '../format'
import { slotOf } from '../lib/enclosure'
import {
  estimatePool,
  fittingLayout,
  misfit,
  POOL_LAYOUTS,
  type Protection,
  protectionOf,
} from '../lib/poolEstimate'
import PlanReview from './PlanReview.vue'

const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ started: [] }>()

const { t } = useI18n()
const jobs = useJobsStore()
const { data: disks } = useDisks()
const { data: enclosure } = useEnclosure()

/** Disks and layout, then the name, then the plan to confirm. */
const STEPS = ['disks', 'name', 'review'] as const
const PROTECTION_TONES: Record<Protection, string> = {
  none: 'text-destructive',
  medium: 'text-amber-700 dark:text-amber-400',
  strong: 'text-emerald-700 dark:text-emerald-400',
  very_strong: 'text-emerald-700 dark:text-emerald-400',
}
/** The parts of the chosen disks' space, as the bar shows them. */
const PARTS = [
  { part: 'usable', colour: 'bg-emerald-500' },
  { part: 'protection', colour: 'bg-primary' },
  { part: 'unused', colour: 'bg-muted-foreground/30' },
] as const
/** A figure on a layout's card: beside its label on phones, under it from `sm`. */
const STAT =
  'flex items-baseline justify-between gap-3 sm:grid sm:content-start sm:justify-start sm:gap-0.5'
/** A card that can be chosen: framed in the primary colour once it is. */
const CARD =
  'relative rounded-lg border transition-colors has-[:focus-visible]:ring-[3px] has-[:focus-visible]:ring-ring/50'
const CHOSEN = 'border-primary bg-primary/5 dark:bg-primary/10'

const step = ref(1)
const name = ref('')
const layout = ref<NewPoolLayout>('mirror')
const chosen = ref<string[]>([])
const plan = ref<PoolPlan | null>(null)
const confirm = ref('')
const error = ref<string | null>(null)
const pending = ref(false)
const heading = useTemplateRef<HTMLElement>('heading')
const layoutList = useTemplateRef<HTMLElement>('layoutList')

/** Disks a pool could go on; the server decides. */
const candidates = computed(() =>
  (disks.value ?? []).filter(
    (disk) =>
      disk.id_link &&
      !disk.read_only &&
      !['system', 'array_member', 'pool_member', 'removable'].includes(disk.usage),
  ),
)
/** The chosen disks' sizes, in the order they were chosen. */
const sizes = computed(() =>
  chosen.value.flatMap((choice) => {
    const disk = candidates.value.find((candidate) => candidate.name === choice)
    return disk ? [disk.size_bytes] : []
  }),
)
const layouts = computed(() =>
  POOL_LAYOUTS.map((option) => {
    const why = chosen.value.length ? misfit(option, chosen.value.length) : null
    const estimate = estimatePool(option, sizes.value)
    return {
      layout: option,
      disabled: why !== null,
      hint: !why
        ? null
        : why.reason === 'exactly'
          ? t('storage.create.misfit.exactly', why.minimum)
          : why.reason === 'tooFew'
            ? t('storage.create.misfit.tooFew', { count: why.minimum })
            : t('storage.create.misfit.tooMany', { count: why.maximum }),
      estimate,
      protection: estimate ? protectionOf(estimate.tolerates) : null,
    }
  }),
)
const estimate = computed(() => estimatePool(layout.value, sizes.value))
const canContinue = computed(() => estimate.value !== null)
/** Shares of the chosen disks' total space, in percent. */
const shares = computed(() => {
  const total = sizes.value.reduce((sum, size) => sum + size, 0)
  if (!estimate.value || !total) return null
  const percent = (bytes: number) => `${(bytes / total) * 100}%`
  return {
    usable: percent(estimate.value.usable),
    protection: percent(estimate.value.protection),
    unused: percent(estimate.value.unused),
  }
})

watch(open, (isOpen) => {
  if (!isOpen) return
  step.value = 1
  name.value = ''
  layout.value = 'mirror'
  chosen.value = []
  plan.value = null
  confirm.value = ''
  error.value = null
})

// A disk that is no longer free cannot stay chosen.
watch(candidates, (list) => {
  const kept = chosen.value.filter((choice) => list.some((disk) => disk.name === choice))
  if (kept.length !== chosen.value.length) chosen.value = kept
})

// A layout the chosen disks do not fit gives way to one they do.
watch(
  () => chosen.value.length,
  async (count) => {
    const next = fittingLayout(layout.value, count)
    if (next === layout.value) return
    layout.value = next
    await nextTick()
    reveal(next)
  },
)

/**
 * Scrolls the layout list, where it scrolls on its own, to show `option`;
 * the dialog itself stays put, so the disk just ticked stays in view.
 */
function reveal(option: NewPoolLayout): void {
  const list = layoutList.value
  const card = list?.querySelector<HTMLElement>(`[data-layout="${option}"]`)
  if (!list || !card) return
  const top = card.offsetTop
  const bottom = top + card.offsetHeight
  if (top < list.scrollTop) list.scrollTop = top
  else if (bottom > list.scrollTop + list.clientHeight) {
    list.scrollTop = bottom - list.clientHeight
  }
}

// Each step starts at its heading, the name step at its field.
watch(step, async (now) => {
  await nextTick()
  if (now === 2) document.getElementById('pool-name')?.focus()
  else heading.value?.focus()
})

const isChosen = (disk: Disk) => chosen.value.includes(disk.name)
const hasData = (disk: Disk) => disk.usage === 'foreign' || disk.usage === 'volume'
const slotLabel = (disk: Disk) => slotOf(enclosure.value, disk.name)?.label

function back(): void {
  error.value = null
  if (step.value === 3) plan.value = null
  step.value -= 1
}

async function review(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    plan.value = await planPool(name.value.trim(), layout.value, chosen.value)
    confirm.value = ''
    step.value = 3
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}

async function create(): Promise<void> {
  if (!plan.value || confirm.value.trim() !== plan.value.name) return
  pending.value = true
  error.value = null
  try {
    jobs.update(await createPool(plan.value.token, confirm.value))
    emit('started')
    open.value = false
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <StepDialog
    v-model:open="open"
    :title="t('storage.create.title')"
    :close-label="t('desktop.close')"
    :steps="STEPS.map((name) => t(`storage.create.steps.${name}`))"
    :step="step - 1"
    wide
  >
    <h3 ref="heading" tabindex="-1" class="sr-only">
      {{ t(`storage.create.steps.${STEPS[step - 1]}`) }}
    </h3>

    <div
      v-if="step === 1"
      class="grid gap-6 md:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]"
      data-testid="pool-step-disks"
    >
      <section class="grid content-start gap-3" aria-labelledby="pool-disks-heading">
        <div class="grid gap-0.5">
          <h4 id="pool-disks-heading" class="text-sm font-medium">
            {{ t('storage.create.disks') }}
          </h4>
          <p class="text-xs text-muted-foreground">{{ t('storage.create.description') }}</p>
        </div>
        <p v-if="!candidates.length" class="text-sm text-muted-foreground">
          {{ t('storage.create.noDisks') }}
        </p>
        <div
          v-else
          role="group"
          aria-labelledby="pool-disks-heading"
          class="-m-[3px] grid content-start gap-2 p-[3px] md:max-h-[min(30rem,calc(100dvh-17rem))] md:overflow-y-auto"
        >
          <label
            v-for="disk in candidates"
            :key="disk.name"
            :class="
              cn(
                CARD,
                'flex cursor-pointer items-center gap-3 p-3 text-sm',
                isChosen(disk) ? CHOSEN : 'hover:bg-accent/60',
              )
            "
          >
            <input v-model="chosen" type="checkbox" :value="disk.name" class="sr-only" />
            <span
              v-if="isChosen(disk)"
              class="absolute -top-px -left-px grid size-4 place-items-center rounded-tl-lg rounded-br-md bg-primary text-primary-foreground"
              aria-hidden="true"
            >
              <Check class="size-3" :stroke-width="3" />
            </span>
            <span
              class="grid size-10 shrink-0 place-items-center rounded-md bg-muted text-muted-foreground"
              aria-hidden="true"
            >
              <component
                :is="disk.media === 'rotational' ? HardDrive : MemoryStick"
                class="size-5"
              />
            </span>
            <span class="grid min-w-0 flex-1 gap-0.5">
              <span class="flex min-w-0 items-center gap-2">
                <span class="shrink-0 font-semibold whitespace-nowrap tabular-nums">
                  {{ formatBytes(disk.size_bytes) }}
                </span>
                <Badge v-if="hasData(disk)" variant="destructive" class="min-w-0 shrink">
                  <span class="truncate">{{ usageLabel(disk) }}</span>
                </Badge>
                <span class="ml-auto shrink-0 text-xs text-muted-foreground">
                  <template v-if="slotLabel(disk)">{{ slotLabel(disk) }} · </template>
                  <span class="font-mono">{{ disk.name }}</span>
                </span>
              </span>
              <span class="flex min-w-0 gap-2 text-xs text-muted-foreground">
                <span class="shrink-0">
                  {{ t(`storage.create.media.${disk.media}`) }} ·
                  {{ t(`storage.transport.${disk.transport}`) }}
                </span>
                <span class="ml-auto min-w-0 truncate">
                  {{ disk.model ?? t('storage.unknownModel') }}
                </span>
              </span>
            </span>
          </label>
        </div>
      </section>

      <section class="grid content-start gap-3" aria-labelledby="pool-layout-heading">
        <h4 id="pool-layout-heading" class="text-sm font-medium">
          {{ t('storage.create.layout') }}
        </h4>
        <div
          ref="layoutList"
          role="radiogroup"
          aria-labelledby="pool-layout-heading"
          class="relative -m-[3px] grid content-start gap-2 p-[3px] md:max-h-[min(30rem,calc(100dvh-17rem))] md:overflow-y-auto"
        >
          <label
            v-for="option in layouts"
            :key="option.layout"
            :data-layout="option.layout"
            :class="
              cn(
                CARD,
                'grid gap-3 p-3 text-sm',
                option.layout === layout
                  ? CHOSEN
                  : option.disabled
                    ? 'cursor-not-allowed opacity-50'
                    : 'cursor-pointer hover:bg-accent/60',
              )
            "
          >
            <input
              v-model="layout"
              type="radio"
              name="pool-layout"
              :value="option.layout"
              :disabled="option.disabled"
              :aria-labelledby="`pool-layout-${option.layout}`"
              :aria-describedby="
                option.hint
                  ? `pool-layout-${option.layout}-hint pool-layout-${option.layout}-details`
                  : `pool-layout-${option.layout}-details`
              "
              class="sr-only"
            />
            <span class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5">
              <span :id="`pool-layout-${option.layout}`" class="font-semibold">
                {{ t(`storage.create.layouts.${option.layout}.name`) }}
              </span>
              <span
                v-if="option.hint"
                :id="`pool-layout-${option.layout}-hint`"
                class="text-xs text-muted-foreground"
              >
                {{ option.hint }}
              </span>
            </span>
            <span :id="`pool-layout-${option.layout}-details`" class="grid gap-3">
              <span class="grid gap-x-3 gap-y-1 sm:grid-cols-3">
                <span :class="STAT">
                  <span class="text-xs text-muted-foreground">
                    {{ t('storage.create.usable') }}
                  </span>
                  <span class="font-medium tabular-nums">
                    {{ option.estimate ? `≈ ${formatBytes(option.estimate.usable)}` : '–' }}
                  </span>
                </span>
                <span :class="STAT">
                  <span class="text-xs text-muted-foreground">{{ t('storage.create.speed') }}</span>
                  <span v-if="option.estimate" class="text-right font-medium sm:text-left">
                    <span class="whitespace-nowrap">
                      {{ t('storage.create.speedRead', { count: option.estimate.read }) }}
                    </span>
                    ·
                    <span class="whitespace-nowrap">
                      {{ t('storage.create.speedWrite', { count: option.estimate.write }) }}
                    </span>
                  </span>
                  <span v-else class="font-medium">–</span>
                </span>
                <span :class="STAT">
                  <span class="text-xs text-muted-foreground">
                    {{ t('storage.create.protection') }}
                  </span>
                  <span
                    v-if="option.estimate && option.protection"
                    class="grid gap-0.5 text-right sm:text-left"
                  >
                    <span :class="cn('font-medium', PROTECTION_TONES[option.protection])">
                      {{ t(`storage.create.strength.${option.protection}`) }}
                    </span>
                    <span class="text-xs text-muted-foreground">
                      {{ t('storage.create.tolerates', option.estimate.tolerates) }}
                    </span>
                  </span>
                  <span v-else class="font-medium">–</span>
                </span>
              </span>
              <span v-if="option.layout === layout" class="grid gap-1 border-t pt-2 text-xs">
                <span>
                  <span class="font-medium text-emerald-700 dark:text-emerald-400">
                    {{ t('storage.create.pros') }}
                  </span>
                  {{ t(`storage.create.layouts.${option.layout}.pros`) }}
                </span>
                <span>
                  <span class="font-medium text-destructive">{{ t('storage.create.cons') }}</span>
                  {{ t(`storage.create.layouts.${option.layout}.cons`) }}
                </span>
              </span>
            </span>
          </label>
        </div>
        <p class="text-xs text-muted-foreground">{{ t('storage.create.estimateNote') }}</p>
      </section>
    </div>

    <form
      v-else-if="step === 2"
      id="pool-name-form"
      class="mx-auto grid w-full max-w-xl gap-4"
      @submit.prevent="review"
    >
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div class="grid gap-2">
        <Label for="pool-name">{{ t('storage.create.name') }}</Label>
        <Input
          id="pool-name"
          v-model="name"
          autocomplete="off"
          autocapitalize="none"
          spellcheck="false"
          maxlength="32"
          required
        />
        <p class="text-xs text-muted-foreground">{{ t('storage.create.nameHint') }}</p>
      </div>
      <dl
        class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 rounded-lg border bg-muted/30 px-4 py-3 text-sm"
      >
        <dt class="text-muted-foreground">{{ t('storage.create.layout') }}</dt>
        <dd>{{ t(`storage.create.layouts.${layout}.name`) }}</dd>
        <dt class="text-muted-foreground">{{ t('storage.create.disks') }}</dt>
        <dd>
          {{ t('storage.create.diskCount', chosen.length) }}
          <span class="font-mono text-xs text-muted-foreground">({{ chosen.join(', ') }})</span>
        </dd>
        <dt class="text-muted-foreground">{{ t('storage.create.estimatedUsable') }}</dt>
        <dd class="tabular-nums">{{ estimate ? formatBytes(estimate.usable) : '–' }}</dd>
      </dl>
    </form>

    <form
      v-else-if="plan"
      id="pool-create-form"
      class="mx-auto grid w-full max-w-xl gap-4"
      @submit.prevent="create"
    >
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 text-sm">
        <dt class="text-muted-foreground">{{ t('storage.create.name') }}</dt>
        <dd class="font-mono text-dense font-medium">{{ plan.name }}</dd>
        <dt class="text-muted-foreground">{{ t('storage.create.layout') }}</dt>
        <dd>{{ t(`storage.create.layouts.${layout}.name`) }}</dd>
        <dt class="text-muted-foreground">{{ t('storage.create.capacity') }}</dt>
        <dd>{{ formatBytes(plan.capacity_bytes) }}</dd>
        <dt class="text-muted-foreground">{{ t('storage.create.safety') }}</dt>
        <dd>{{ t('storage.redundancy', plan.redundancy) }}</dd>
        <dt class="text-muted-foreground">{{ t('storage.create.path') }}</dt>
        <dd class="font-mono text-xs leading-5">{{ plan.mount_point }}</dd>
      </dl>
      <PlanReview :disks="plan.disks" :warnings="plan.warnings" />
      <div class="grid gap-2">
        <Label for="pool-confirm">{{ t('storage.create.confirm', { name: plan.name }) }}</Label>
        <Input id="pool-confirm" v-model="confirm" autocomplete="off" spellcheck="false" required />
      </div>
    </form>

    <template #footer>
      <div
        v-if="step === 1"
        class="flex min-w-0 basis-full items-center gap-3 sm:mr-auto sm:flex-1 sm:basis-auto sm:pr-4"
      >
        <p id="pool-estimate-heading" class="w-20 shrink-0 text-xs leading-tight font-medium">
          {{ t('storage.create.estimated') }}
        </p>
        <div class="grid min-w-0 flex-1 gap-1.5">
          <div class="flex h-2 overflow-hidden rounded-full bg-muted" aria-hidden="true">
            <template v-if="shares">
              <div
                v-for="{ part, colour } in PARTS"
                :key="part"
                :class="colour"
                :style="{ width: shares[part] }"
              />
            </template>
          </div>
          <ul
            aria-labelledby="pool-estimate-heading"
            class="flex flex-wrap gap-x-4 gap-y-0.5 text-xs text-muted-foreground"
          >
            <li v-for="{ part, colour } in PARTS" :key="part" class="flex items-center gap-1.5">
              <span :class="cn('size-2 rounded-full', colour)" aria-hidden="true" />
              {{ t(`storage.create.legend.${part}`) }}
              <span class="font-medium text-foreground tabular-nums">
                {{ estimate ? formatBytes(estimate[part]) : '–' }}
              </span>
            </li>
          </ul>
        </div>
      </div>
      <Button v-else type="button" variant="outline" class="mr-auto" @click="back">
        {{ t('desktop.back') }}
      </Button>
      <Button type="button" variant="outline" @click="open = false">
        {{ t('desktop.cancel') }}
      </Button>
      <Button v-if="step === 1" type="button" :disabled="!canContinue" @click="step = 2">
        {{ t('storage.create.next') }}
      </Button>
      <Button
        v-else-if="step === 2"
        type="submit"
        form="pool-name-form"
        :disabled="pending || !name.trim()"
      >
        {{ t('storage.create.next') }}
      </Button>
      <Button
        v-else
        type="submit"
        form="pool-create-form"
        variant="destructive"
        :disabled="pending || !plan || confirm.trim() !== plan.name"
      >
        {{ t('storage.create.submit') }}
      </Button>
    </template>
  </StepDialog>
</template>
