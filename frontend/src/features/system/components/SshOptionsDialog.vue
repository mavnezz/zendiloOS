<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { RadioGroup, RadioGroupItem } from '@/shared/ui/radio-group'

import type { SshDraft } from '../lib/ssh'

const props = defineProps<{ ssh: SshDraft; saving: boolean; error?: string | null }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ save: [ssh: SshDraft] }>()

const { t } = useI18n()
/** A copy: nothing changes until saved. */
const draft = ref<SshDraft>({ ...props.ssh })
watch(open, (isOpen) => {
  if (isOpen) draft.value = { ...props.ssh }
})
</script>

<!-- Who may log in how: root, and passwords. Everything else stays as Debian has it. -->
<template>
  <StepDialog
    v-model:open="open"
    :title="t('terminal.advanced.title')"
    :close-label="t('desktop.close')"
  >
    <Alert v-if="error" variant="destructive" class="mb-3">{{ error }}</Alert>
    <div class="grid gap-6 py-1">
      <section class="grid gap-2">
        <h3 class="text-sm font-medium">{{ t('terminal.advanced.rootLogin') }}</h3>
        <RadioGroup v-model="draft.root_login" :label="t('terminal.advanced.rootLogin')">
          <RadioGroupItem value="system">{{ t('terminal.advanced.asSystem') }}</RadioGroupItem>
          <RadioGroupItem value="no">{{ t('terminal.advanced.rootNo') }}</RadioGroupItem>
          <RadioGroupItem value="key_only">{{ t('terminal.advanced.rootKeyOnly') }}</RadioGroupItem>
          <RadioGroupItem value="yes">{{ t('terminal.advanced.rootYes') }}</RadioGroupItem>
        </RadioGroup>
        <p class="text-xs text-muted-foreground">{{ t('terminal.advanced.rootHint') }}</p>
      </section>
      <section class="grid gap-2">
        <h3 class="text-sm font-medium">{{ t('terminal.advanced.passwordLogin') }}</h3>
        <RadioGroup v-model="draft.password_login" :label="t('terminal.advanced.passwordLogin')">
          <RadioGroupItem value="system">{{ t('terminal.advanced.asSystem') }}</RadioGroupItem>
          <RadioGroupItem value="yes">{{ t('terminal.advanced.passwordYes') }}</RadioGroupItem>
          <RadioGroupItem value="no">{{ t('terminal.advanced.passwordNo') }}</RadioGroupItem>
        </RadioGroup>
        <p class="text-xs text-muted-foreground">{{ t('terminal.advanced.passwordHint') }}</p>
      </section>
    </div>

    <template #footer>
      <Button variant="ghost" :disabled="saving" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button :disabled="saving" @click="emit('save', { ...draft })">{{
        t('terminal.advanced.save')
      }}</Button>
    </template>
  </StepDialog>
</template>
