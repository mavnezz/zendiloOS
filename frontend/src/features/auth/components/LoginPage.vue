<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Card, CardContent } from '@/shared/ui/card'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { login } from '../api'
import { useSessionStore } from '../store'
import AuthFrame from './AuthFrame.vue'

const { t } = useI18n()
const router = useRouter()
const session = useSessionStore()

const name = ref('')
const password = ref('')
const error = ref<string | null>(null)
const pending = ref(false)

async function submit(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    session.start(await login(name.value, password.value))
    await router.push({ name: 'desktop' })
  } catch (failure) {
    error.value = problemMessage(failure)
    password.value = ''
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <AuthFrame>
    <Card>
      <CardContent class="pt-6">
        <form class="grid gap-5" @submit.prevent="submit">
          <Alert v-if="error" variant="destructive">{{ error }}</Alert>
          <div class="grid gap-2">
            <Label for="login-name">{{ t('login.name') }}</Label>
            <Input
              id="login-name"
              v-model="name"
              autocomplete="username"
              autocapitalize="none"
              spellcheck="false"
              required
              autofocus
            />
          </div>
          <div class="grid gap-2">
            <Label for="login-password">{{ t('login.password') }}</Label>
            <Input
              id="login-password"
              v-model="password"
              type="password"
              autocomplete="current-password"
              required
            />
          </div>
          <Button type="submit" class="w-full" :disabled="pending">{{ t('login.submit') }}</Button>
        </form>
      </CardContent>
    </Card>
  </AuthFrame>
</template>
