<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useShareActions } from '../../sharing'

/** A folder directly on a volume, with all it holds: confirmed by its name. */
const props = defineProps<{ folder: { id: number; name: string; path: string } | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useShareActions()
const notifications = useNotifications()
const confirm = ref('')
const error = ref<string | null>(null)

watch(open, (isOpen) => {
  if (isOpen) {
    confirm.value = ''
    error.value = null
  }
})

async function remove(): Promise<void> {
  const folder = props.folder
  if (!folder) return
  error.value = null
  try {
    await actions.remove(folder.id, confirm.value)
    notifications.notify('info', t('shares.deleting', { name: folder.name }))
    open.value = false
  } catch (failure) {
    error.value = problemMessage(failure)
  }
}
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="t('explorer.deleteFolder.title', { name: folder?.name })"
    :description="t('shares.deleteDescription', { path: folder?.path })"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="remove">
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div class="grid gap-2">
        <Label for="folder-confirm">{{
          t('storage.create.confirm', { name: folder?.name })
        }}</Label>
        <Input id="folder-confirm" v-model="confirm" autocomplete="off" spellcheck="false" />
      </div>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" variant="destructive" :disabled="confirm.trim() !== folder?.name">
          {{ t('explorer.deleteFolder.submit') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
