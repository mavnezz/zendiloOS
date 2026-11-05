<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Account } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useAccountActions } from '../api'

const MIN_PASSWORD_CHARS = 10

const props = defineProps<{ account: Account }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useAccountActions()
const notifications = useNotifications()

const password = ref('')
const error = ref<string | null>(null)
const pending = ref(false)

watch(open, () => {
  password.value = ''
  error.value = null
})

async function submit(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    await actions.resetPassword(props.account.id, password.value)
    notifications.notify('success', t('users.passwordReset', { name: props.account.name }))
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
    :title="t('users.resetTitle', { name: account.name })"
    :description="t('users.resetDescription')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="submit">
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div class="grid gap-2">
        <Label for="reset-password">{{ t('settings.newPassword') }}</Label>
        <Input
          id="reset-password"
          v-model="password"
          type="password"
          autocomplete="new-password"
          :minlength="MIN_PASSWORD_CHARS"
          required
        />
        <p class="text-xs text-muted-foreground">
          {{ t('settings.passwordHint', { min: MIN_PASSWORD_CHARS }) }}
        </p>
      </div>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="pending">{{ t('users.resetAction') }}</Button>
      </div>
    </form>
  </ModalDialog>
</template>
