<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Disk, NewPoolLayout, PoolPlan } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { createPool, planPool, useDisks } from '../api'
import { usageLabel } from '../format'
import PlanReview from './PlanReview.vue'

const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ started: [] }>()

const { t } = useI18n()
const { data: disks } = useDisks()

const LAYOUTS: { layout: NewPoolLayout; minimum: number; maximum: number }[] = [
  { layout: 'mirror', minimum: 2, maximum: 12 },
  { layout: 'raidz1', minimum: 3, maximum: 12 },
  { layout: 'raidz2', minimum: 4, maximum: 12 },
  { layout: 'raidz3', minimum: 5, maximum: 12 },
  { layout: 'single', minimum: 1, maximum: 1 },
]

const name = ref('')
const layout = ref<NewPoolLayout>('mirror')
const chosen = ref<string[]>([])
const plan = ref<PoolPlan | null>(null)
const confirm = ref('')
const error = ref<string | null>(null)
const pending = ref(false)

/** Disks a pool could go on; the server decides. */
const candidates = computed(() =>
  (disks.value ?? []).filter(
    (disk) =>
      disk.id_link &&
      !disk.read_only &&
      !['system', 'array_member', 'pool_member'].includes(disk.usage),
  ),
)
const fits = (option: (typeof LAYOUTS)[number]) =>
  chosen.value.length >= option.minimum && chosen.value.length <= option.maximum

watch(open, (isOpen) => {
  if (!isOpen) return
  name.value = ''
  layout.value = 'mirror'
  chosen.value = []
  plan.value = null
  confirm.value = ''
  error.value = null
})

watch(chosen, () => {
  const current = LAYOUTS.find((option) => option.layout === layout.value)
  if (current && !fits(current)) {
    layout.value = LAYOUTS.find(fits)?.layout ?? layout.value
  }
})

const hasData = (disk: Disk) => disk.usage === 'foreign' || disk.usage === 'volume'

async function review(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    plan.value = await planPool(name.value.trim(), layout.value, chosen.value)
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}

async function create(): Promise<void> {
  if (!plan.value) return
  pending.value = true
  error.value = null
  try {
    await createPool(plan.value.token, confirm.value)
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
  <ModalDialog
    v-model:open="open"
    :title="t('storage.create.title')"
    :description="plan ? undefined : t('storage.create.description')"
    :close-label="t('desktop.close')"
  >
    <Alert v-if="error" variant="destructive">{{ error }}</Alert>

    <form v-if="!plan" class="grid gap-4" @submit.prevent="review">
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

      <fieldset class="grid gap-2">
        <legend class="mb-1 text-sm font-medium">{{ t('storage.create.disks') }}</legend>
        <p v-if="!candidates.length" class="text-sm text-muted-foreground">
          {{ t('storage.create.noDisks') }}
        </p>
        <label
          v-for="disk in candidates"
          :key="disk.name"
          class="flex items-center gap-2 rounded-lg border px-3 py-2 text-sm hover:bg-accent/60"
        >
          <input
            v-model="chosen"
            type="checkbox"
            :value="disk.name"
            class="size-4 accent-primary"
          />
          <span class="font-medium">{{ disk.name }}</span>
          <span class="min-w-0 flex-1 truncate text-muted-foreground">
            {{ disk.model ?? t('storage.unknownModel') }} · {{ formatBytes(disk.size_bytes) }}
          </span>
          <span v-if="hasData(disk)" class="shrink-0 text-xs text-destructive">{{
            usageLabel(disk)
          }}</span>
        </label>
      </fieldset>

      <fieldset class="grid gap-2">
        <legend class="mb-1 text-sm font-medium">{{ t('storage.create.layout') }}</legend>
        <label
          v-for="option in LAYOUTS"
          :key="option.layout"
          class="flex items-start gap-2 rounded-lg border px-3 py-2 text-sm"
          :class="fits(option) ? 'hover:bg-accent/60' : 'opacity-50'"
        >
          <input
            v-model="layout"
            type="radio"
            :value="option.layout"
            :disabled="!fits(option)"
            class="mt-0.5 size-4 accent-primary"
          />
          <span class="grid gap-0.5">
            <span class="font-medium">{{ t(`storage.create.layouts.${option.layout}.name`) }}</span>
            <span class="text-xs text-muted-foreground">
              {{ t(`storage.create.layouts.${option.layout}.description`) }}
            </span>
          </span>
        </label>
      </fieldset>

      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="pending || !chosen.length || !name.trim()">
          {{ t('storage.create.review') }}
        </Button>
      </div>
    </form>

    <form v-else class="grid gap-4" @submit.prevent="create">
      <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 text-sm">
        <dt class="text-muted-foreground">{{ t('storage.create.name') }}</dt>
        <dd class="font-medium">{{ plan.name }}</dd>
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
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="plan = null">{{
          t('desktop.back')
        }}</Button>
        <Button
          type="submit"
          variant="destructive"
          :disabled="pending || confirm.trim() !== plan.name"
        >
          {{ t('storage.create.submit') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
