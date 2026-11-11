<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatPercent } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { Meter } from '@/shared/ui/meter'

import { useImageActions } from '../api'
import { imageNameValid } from '../lib/vms'

/** An ISO from this computer into the library, at most `max` bytes. */
const props = defineProps<{ max: number }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useImageActions()

const file = ref<File | null>(null)
const name = ref('')
const progress = ref<number | null>(null)
const error = ref<string | null>(null)
let stop: AbortController | null = null

watch(open, (isOpen) => {
  if (isOpen) {
    file.value = null
    name.value = ''
    progress.value = null
    error.value = null
  } else {
    stop?.abort()
  }
})
onBeforeUnmount(() => stop?.abort())

function choose(event: Event): void {
  const chosen = (event.target as HTMLInputElement).files?.[0] ?? null
  file.value = chosen
  if (chosen) name.value = chosen.name
}

const tooLarge = computed(() => (file.value?.size ?? 0) > props.max)
const valid = computed(
  () => file.value !== null && file.value.size > 0 && !tooLarge.value && imageNameValid(name.value),
)

async function upload(): Promise<void> {
  const chosen = file.value
  if (!chosen || !valid.value) return
  error.value = null
  progress.value = 0
  stop = new AbortController()
  try {
    await actions.upload(
      chosen,
      name.value.trim(),
      (fraction) => (progress.value = fraction),
      stop.signal,
    )
    open.value = false
  } catch (problem) {
    if (open.value) error.value = problemMessage(problem)
  } finally {
    progress.value = null
    stop = null
  }
}
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="t('vms.images.uploadDialog.title')"
    :close-label="t('desktop.close')"
  >
    <form id="vm-image-upload" class="grid gap-4" @submit.prevent="upload">
      <div class="grid gap-2">
        <Label for="vm-image-upload-file">{{ t('vms.images.uploadDialog.file') }}</Label>
        <input
          id="vm-image-upload-file"
          type="file"
          accept=".iso"
          class="text-sm file:mr-3 file:rounded-md file:border file:bg-background file:px-3 file:py-1 file:text-sm"
          :disabled="progress !== null"
          @change="choose"
        />
        <p v-if="tooLarge" class="text-xs text-destructive">
          {{ t('vms.images.uploadDialog.tooLarge', { max: formatBytes(max) }) }}
        </p>
      </div>
      <div class="grid gap-2">
        <Label for="vm-image-upload-name">{{ t('vms.images.uploadDialog.name') }}</Label>
        <Input
          id="vm-image-upload-name"
          v-model="name"
          maxlength="128"
          autocomplete="off"
          :disabled="!file || progress !== null"
          :aria-invalid="file !== null && !imageNameValid(name)"
        />
        <p v-if="file && !imageNameValid(name)" class="text-xs text-destructive">
          {{ t('vms.images.invalidName') }}
        </p>
      </div>
      <p class="text-xs text-muted-foreground">
        {{ t('vms.images.uploadDialog.hint', { max: formatBytes(max) }) }}
      </p>
      <div v-if="progress !== null" class="grid gap-1.5" data-testid="vm-image-progress">
        <span class="text-sm">{{
          t('vms.images.uploadDialog.uploading', { percent: formatPercent(progress * 100) })
        }}</span>
        <Meter :value="progress" kind="progress" :label="t('vms.images.uploadDialog.title')" />
      </div>
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="progress !== null || !valid">
          {{ t('vms.images.uploadDialog.submit') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
