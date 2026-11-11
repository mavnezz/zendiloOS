<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useVmActions } from '../api'

const props = defineProps<{ vm: VirtualMachine }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useVmActions()
const confirm = ref('')
const error = ref<string | null>(null)
const pending = ref(false)

watch(open, () => {
  confirm.value = ''
  error.value = null
})

async function remove(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    await actions.remove(props.vm.id, confirm.value)
    open.value = false
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}
</script>

<!-- Deleting a VM, its disk with it: its name typed again. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('vms.deleteDialog.title', { name: vm.name })"
    :description="t('vms.deleteDialog.description')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="remove">
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div class="grid gap-2">
        <Label for="vm-delete-confirm">{{
          t('vms.deleteDialog.confirm', { name: vm.name })
        }}</Label>
        <Input
          id="vm-delete-confirm"
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
          :disabled="pending || confirm.trim() !== vm.name"
        >
          {{ t('vms.deleteDialog.submit') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
