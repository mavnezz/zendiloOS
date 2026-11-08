<script setup lang="ts">
import { LoaderCircle } from 'lucide-vue-next'
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
  /** What the name must look like, under the field. */
  hint?: string
  /** It is being made or renamed: nothing to change meanwhile. */
  busy?: boolean
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
    <form
      class="grid gap-4"
      :aria-busy="busy"
      @submit.prevent="!busy && valid(name.trim()) && emit('submit', name.trim())"
    >
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div class="grid gap-2">
        <Label for="file-name">{{ t('files.name') }}</Label>
        <Input
          id="file-name"
          v-model="name"
          autocomplete="off"
          spellcheck="false"
          maxlength="255"
          :disabled="busy"
        />
        <p v-if="hint" class="text-xs text-muted-foreground">{{ hint }}</p>
        <p v-if="name.includes('/')" class="text-xs text-destructive">{{ t('files.noSlash') }}</p>
        <p v-else-if="name.trim().endsWith('.')" class="text-xs text-destructive">
          {{ t('files.noTrailingDot') }}
        </p>
      </div>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" :disabled="busy" @click="open = false">{{
          t('desktop.cancel')
        }}</Button>
        <Button type="submit" :disabled="busy || !valid(name.trim())">
          <LoaderCircle v-if="busy" class="animate-spin" />
          {{ submitLabel }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
