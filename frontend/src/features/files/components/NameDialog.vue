<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

const props = defineProps<{
  title: string
  initial: string
  submitLabel: string
  error: string | null
}>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ submit: [name: string] }>()

const { t } = useI18n()
const name = ref('')

watch(open, (isOpen) => {
  if (isOpen) name.value = props.initial
})

const CONTROL = /[\u0000-\u001f\u007f]/

/**
 * What the server refuses for new names anyway: also no dot at the end and
 * no control characters, which Windows could not reach. Spaces at either
 * end are trimmed.
 */
function valid(value: string): boolean {
  return value !== '' && !value.includes('/') && !value.endsWith('.') && !CONTROL.test(value)
}
</script>

<!-- Asks for the name of a new folder or a new name for an entry. -->
<template>
  <ModalDialog v-model:open="open" :title="title" :close-label="t('desktop.close')">
    <form class="grid gap-4" @submit.prevent="valid(name.trim()) && emit('submit', name.trim())">
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div class="grid gap-2">
        <Label for="file-name">{{ t('files.name') }}</Label>
        <Input
          id="file-name"
          v-model="name"
          autocomplete="off"
          spellcheck="false"
          maxlength="255"
        />
        <p v-if="name.includes('/')" class="text-xs text-destructive">{{ t('files.noSlash') }}</p>
        <p v-else-if="name.trim().endsWith('.')" class="text-xs text-destructive">
          {{ t('files.noTrailingDot') }}
        </p>
      </div>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">{{
          t('desktop.cancel')
        }}</Button>
        <Button type="submit" :disabled="!valid(name.trim())">{{ submitLabel }}</Button>
      </div>
    </form>
  </ModalDialog>
</template>
