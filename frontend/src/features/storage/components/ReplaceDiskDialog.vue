<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PoolDevice, ReplacePlan } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'

import { planReplace, replaceDisk, useDisks } from '../api'
import PlanReview from './PlanReview.vue'

const props = defineProps<{ pool: string; device: PoolDevice }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ started: [] }>()

const { t } = useI18n()
const { data: disks } = useDisks()

const disk = ref('')
const plan = ref<ReplacePlan | null>(null)
const understood = ref(false)
const error = ref<string | null>(null)
const pending = ref(false)

const candidates = computed(() =>
  (disks.value ?? []).filter(
    (d) =>
      d.id_link && !d.read_only && !['system', 'array_member', 'pool_member'].includes(d.usage),
  ),
)

watch(open, (isOpen) => {
  if (!isOpen) return
  disk.value = ''
  plan.value = null
  understood.value = false
  error.value = null
})

async function review(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    plan.value = await planReplace(props.pool, props.device.guid, disk.value)
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}

async function replace(): Promise<void> {
  if (!plan.value) return
  pending.value = true
  error.value = null
  try {
    await replaceDisk(props.pool, plan.value.token)
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
    :title="t('storage.replaceDisk.title', { device: device.disk ?? device.name })"
    :description="t('storage.replaceDisk.description')"
    :close-label="t('desktop.close')"
  >
    <Alert v-if="error" variant="destructive">{{ error }}</Alert>
    <form v-if="!plan" class="grid gap-4" @submit.prevent="review">
      <p v-if="!candidates.length" class="text-sm text-muted-foreground">
        {{ t('storage.create.noDisks') }}
      </p>
      <label
        v-for="candidate in candidates"
        :key="candidate.name"
        class="flex items-center gap-2 rounded-lg border px-3 py-2 text-sm hover:bg-accent/60"
      >
        <input v-model="disk" type="radio" :value="candidate.name" class="size-4 accent-primary" />
        <span class="font-medium">{{ candidate.name }}</span>
        <span class="min-w-0 flex-1 truncate text-muted-foreground">
          {{ candidate.model ?? t('storage.unknownModel') }} ·
          {{ formatBytes(candidate.size_bytes) }}
        </span>
      </label>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="pending || !disk">{{ t('storage.create.review') }}</Button>
      </div>
    </form>
    <form v-else class="grid gap-4" @submit.prevent="replace">
      <PlanReview :disks="[plan.disk]" :warnings="plan.warnings" />
      <label class="flex items-start gap-2 text-sm">
        <input v-model="understood" type="checkbox" class="mt-0.5 size-4 accent-primary" />
        {{ t('storage.replaceDisk.understood', { disk: plan.disk.name }) }}
      </label>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="plan = null">{{
          t('desktop.back')
        }}</Button>
        <Button type="submit" variant="destructive" :disabled="pending || !understood">
          {{ t('storage.replaceDisk.submit') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
