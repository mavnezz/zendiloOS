<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { ApiProblem } from '@/shared/api/client'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/shared/ui/card'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { completeSetup } from '../api'
import { useSessionStore } from '../store'
import AuthFrame from './AuthFrame.vue'

/** Mirrors the server's rule; the server decides. */
const MIN_PASSWORD_CHARS = 10

const { t } = useI18n()
const router = useRouter()
const session = useSessionStore()

const setupCode = ref('')
const name = ref('')
const password = ref('')
const passwordRepeat = ref('')
const error = ref<string | null>(null)
const completedElsewhere = ref(false)
const pending = ref(false)

const passwordsDiffer = computed(
  () => passwordRepeat.value !== '' && password.value !== passwordRepeat.value,
)

async function submit(): Promise<void> {
  if (password.value !== passwordRepeat.value) return
  pending.value = true
  error.value = null
  try {
    session.start(
      await completeSetup({
        setup_code: setupCode.value,
        name: name.value,
        password: password.value,
      }),
    )
    await router.push({ name: 'desktop' })
  } catch (failure) {
    error.value = problemMessage(failure)
    completedElsewhere.value = failure instanceof ApiProblem && failure.code === 'setup_completed'
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <AuthFrame>
    <Card>
      <CardHeader>
        <CardTitle class="text-xl">{{ t('setup.title') }}</CardTitle>
        <CardDescription>{{ t('setup.intro') }}</CardDescription>
      </CardHeader>
      <CardContent>
        <form class="grid gap-5" @submit.prevent="submit">
          <Alert v-if="error" variant="destructive" class="flex-col gap-2">
            <span>{{ error }}</span>
            <RouterLink v-if="completedElsewhere" :to="{ name: 'login' }" class="underline">
              {{ t('setup.toLogin') }}
            </RouterLink>
          </Alert>
          <div class="grid gap-2">
            <Label for="setup-code">{{ t('setup.code') }}</Label>
            <Input
              id="setup-code"
              v-model="setupCode"
              class="font-mono tracking-widest uppercase"
              autocomplete="off"
              autocapitalize="characters"
              spellcheck="false"
              placeholder="XXXX-XXXX-XXXX"
              required
              autofocus
            />
            <p class="text-xs text-muted-foreground">{{ t('setup.codeHint') }}</p>
          </div>
          <div class="grid gap-2">
            <Label for="setup-name">{{ t('setup.name') }}</Label>
            <Input
              id="setup-name"
              v-model="name"
              autocomplete="username"
              autocapitalize="none"
              spellcheck="false"
              required
            />
            <p class="text-xs text-muted-foreground">{{ t('setup.nameHint') }}</p>
          </div>
          <div class="grid gap-2">
            <Label for="setup-password">{{ t('setup.password') }}</Label>
            <Input
              id="setup-password"
              v-model="password"
              type="password"
              autocomplete="new-password"
              :minlength="MIN_PASSWORD_CHARS"
              required
            />
            <p class="text-xs text-muted-foreground">
              {{ t('setup.passwordHint', { min: MIN_PASSWORD_CHARS }) }}
            </p>
          </div>
          <div class="grid gap-2">
            <Label for="setup-password-repeat">{{ t('setup.passwordRepeat') }}</Label>
            <Input
              id="setup-password-repeat"
              v-model="passwordRepeat"
              type="password"
              autocomplete="new-password"
              :aria-invalid="passwordsDiffer"
              required
            />
            <p v-if="passwordsDiffer" class="text-xs text-destructive">
              {{ t('setup.passwordsDiffer') }}
            </p>
          </div>
          <Button type="submit" class="w-full" :disabled="pending || passwordsDiffer">
            {{ t('setup.submit') }}
          </Button>
        </form>
      </CardContent>
    </Card>
  </AuthFrame>
</template>
