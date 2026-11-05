<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { changePassword } from '../api'

/** Mirrors the server's rule; the server decides. */
const MIN_PASSWORD_CHARS = 10

const emit = defineEmits<{ changed: [] }>()

const { t } = useI18n()
const notifications = useNotifications()

const current = ref('')
const next = ref('')
const repeat = ref('')
const error = ref<string | null>(null)
const pending = ref(false)

const differ = computed(() => repeat.value !== '' && next.value !== repeat.value)

async function submit(): Promise<void> {
  if (next.value !== repeat.value) return
  pending.value = true
  error.value = null
  try {
    await changePassword(current.value, next.value)
    current.value = next.value = repeat.value = ''
    notifications.notify(
      'success',
      t('settings.passwordChanged'),
      t('settings.passwordChangedHint'),
    )
    emit('changed')
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <form class="grid max-w-sm gap-4" @submit.prevent="submit">
    <Alert v-if="error" variant="destructive">{{ error }}</Alert>
    <div class="grid gap-2">
      <Label for="password-current">{{ t('settings.currentPassword') }}</Label>
      <Input
        id="password-current"
        v-model="current"
        type="password"
        autocomplete="current-password"
        required
      />
    </div>
    <div class="grid gap-2">
      <Label for="password-new">{{ t('settings.newPassword') }}</Label>
      <Input
        id="password-new"
        v-model="next"
        type="password"
        autocomplete="new-password"
        :minlength="MIN_PASSWORD_CHARS"
        required
      />
      <p class="text-xs text-muted-foreground">
        {{ t('settings.passwordHint', { min: MIN_PASSWORD_CHARS }) }}
      </p>
    </div>
    <div class="grid gap-2">
      <Label for="password-repeat">{{ t('settings.repeatPassword') }}</Label>
      <Input
        id="password-repeat"
        v-model="repeat"
        type="password"
        autocomplete="new-password"
        :aria-invalid="differ"
        required
      />
      <p v-if="differ" class="text-xs text-destructive">{{ t('settings.passwordsDiffer') }}</p>
    </div>
    <Button type="submit" class="justify-self-start" :disabled="pending || differ">
      {{ t('settings.changePassword') }}
    </Button>
  </form>
</template>
