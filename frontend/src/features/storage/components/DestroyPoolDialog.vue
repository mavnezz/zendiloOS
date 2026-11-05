<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Pool } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { destroyPool } from '../api'

const props = defineProps<{ pool: Pool }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ started: [] }>()

const { t } = useI18n()
const confirm = ref('')
const error = ref<string | null>(null)
const pending = ref(false)

watch(open, () => {
  confirm.value = ''
  error.value = null
})

async function destroy(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    await destroyPool(props.pool.name, confirm.value)
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
    :title="t('storage.destroy.title', { name: pool.name })"
    :description="t('storage.destroy.description', { path: pool.mount_point ?? '' })"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="destroy">
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div class="grid gap-2">
        <Label for="destroy-confirm">{{ t('storage.create.confirm', { name: pool.name }) }}</Label>
        <Input
          id="destroy-confirm"
          v-model="confirm"
          autocomplete="off"
          spellcheck="false"
          required
        />
      </div>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button
          type="submit"
          variant="destructive"
          :disabled="pending || confirm.trim() !== pool.name"
        >
          {{ t('storage.destroy.submit') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
