<script setup lang="ts">
import { Disc3 } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { FolderPicker, useShareList } from '@/shared/ui/folder-picker'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useImageActions } from '../api'
import { fileName, imageNameValid, isoPick } from '../lib/vms'

/** An ISO from a share into the library, copied in a job. */
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useImageActions()
const jobs = useJobsStore()
const { data: shares } = useShareList()

const picked = ref<{ shareId: number; path: string; shown: string } | null>(null)
const name = ref('')
const picking = ref(false)
const pending = ref(false)
const error = ref<string | null>(null)

watch(open, (isOpen) => {
  if (!isOpen) return
  picked.value = null
  name.value = ''
  error.value = null
})

function pick(path: string): void {
  picked.value = isoPick(path, shares.value)
  if (picked.value) name.value = fileName(picked.value.path)
}

const valid = computed(() => picked.value !== null && imageNameValid(name.value))

async function add(): Promise<void> {
  const choice = picked.value
  if (!choice || !valid.value) return
  pending.value = true
  error.value = null
  try {
    const added = await actions.importFromShare(choice.shareId, choice.path, name.value.trim())
    jobs.update(added.job)
    open.value = false
  } catch (problem) {
    error.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="t('vms.images.addDialog.title')"
    :close-label="t('desktop.close')"
  >
    <form id="vm-image-add" class="grid gap-4" @submit.prevent="add">
      <section class="grid gap-2" aria-labelledby="vm-image-file">
        <h4 id="vm-image-file" class="text-sm font-medium">{{ t('vms.images.addDialog.file') }}</h4>
        <div class="flex flex-wrap items-center gap-2 rounded-lg border p-3 text-sm">
          <Disc3 class="size-4 shrink-0 text-muted-foreground" />
          <span
            v-if="picked"
            class="min-w-0 flex-1 truncate font-mono text-xs"
            data-testid="vm-image-picked"
            >{{ picked.shown }}</span
          >
          <span v-else class="min-w-0 flex-1 text-muted-foreground">{{
            t('vms.images.addDialog.none')
          }}</span>
          <Button type="button" size="sm" variant="outline" @click="picking = true">
            {{ picked ? t('vms.images.addDialog.change') : t('vms.images.addDialog.pick') }}
          </Button>
        </div>
      </section>
      <div class="grid gap-2">
        <Label for="vm-image-name">{{ t('vms.images.addDialog.name') }}</Label>
        <Input
          id="vm-image-name"
          v-model="name"
          maxlength="128"
          autocomplete="off"
          :disabled="!picked"
          :aria-invalid="picked !== null && !imageNameValid(name)"
        />
        <p v-if="picked && !imageNameValid(name)" class="text-xs text-destructive">
          {{ t('vms.images.invalidName') }}
        </p>
      </div>
      <p class="text-xs text-muted-foreground">{{ t('vms.images.addDialog.hint') }}</p>
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="pending || !valid">
          {{ t('vms.images.addDialog.submit') }}
        </Button>
      </div>
    </form>
    <FolderPicker v-model:open="picking" :value="picked?.shown ?? ''" files=".iso" @pick="pick" />
  </ModalDialog>
</template>
