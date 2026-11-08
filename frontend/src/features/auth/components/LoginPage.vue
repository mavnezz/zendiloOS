<script setup lang="ts">
import { nextTick, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { ApiProblem } from '@/shared/api/client'
import type { SecondFactorChallenge, SessionInfo } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Card, CardContent } from '@/shared/ui/card'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { completeSignIn, login } from '../api'
import { useSessionStore } from '../store'
import AuthFrame from './AuthFrame.vue'

const { t } = useI18n()
const router = useRouter()
const session = useSessionStore()

const name = ref('')
const password = ref('')
const error = ref<string | null>(null)
const pending = ref(false)
/** The password checked out; the second factor is asked for. */
const challenge = ref<SecondFactorChallenge | null>(null)
const code = ref('')
/** A recovery code instead of the app's. */
const recovery = ref(false)

async function signedIn(info: SessionInfo): Promise<void> {
  session.start(info)
  await router.push({ name: 'desktop' })
}

async function focusCode(): Promise<void> {
  await nextTick()
  document.getElementById('login-code')?.focus()
}

async function submit(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    const result = await login(name.value, password.value)
    password.value = ''
    if (result.kind === 'signed_in') {
      await signedIn(result.session)
    } else {
      challenge.value = result.challenge
      await focusCode()
    }
  } catch (failure) {
    error.value = problemMessage(failure)
    password.value = ''
  } finally {
    pending.value = false
  }
}

async function verify(): Promise<void> {
  if (!challenge.value) return
  pending.value = true
  error.value = null
  try {
    await signedIn(await completeSignIn(challenge.value.token, code.value))
  } catch (failure) {
    error.value = problemMessage(failure)
    code.value = ''
    // Too slow, or too many wrong codes: the password again.
    if (failure instanceof ApiProblem && failure.code === 'sign_in_expired') challenge.value = null
    else await focusCode()
  } finally {
    pending.value = false
  }
}

function toggleRecovery(): void {
  recovery.value = !recovery.value
  code.value = ''
  error.value = null
  void focusCode()
}

function startOver(): void {
  challenge.value = null
  recovery.value = false
  code.value = ''
  error.value = null
}
</script>

<template>
  <AuthFrame>
    <Card>
      <CardContent class="pt-6">
        <form v-if="!challenge" class="grid gap-5" @submit.prevent="submit">
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

        <form v-else class="grid gap-5" @submit.prevent="verify">
          <Alert v-if="error" variant="destructive">{{ error }}</Alert>
          <div class="grid gap-2">
            <Label for="login-code">
              {{ recovery ? t('login.recoveryCode') : t('login.code') }}
            </Label>
            <Input
              id="login-code"
              v-model="code"
              class="font-mono tracking-widest"
              :inputmode="recovery ? 'text' : 'numeric'"
              :autocomplete="recovery ? 'off' : 'one-time-code'"
              :maxlength="recovery ? 11 : 6"
              autocapitalize="none"
              spellcheck="false"
              required
            />
            <p class="text-xs text-muted-foreground">
              {{ recovery ? t('login.recoveryHint') : t('login.codeHint') }}
            </p>
          </div>
          <Button type="submit" class="w-full" :disabled="pending">{{ t('login.verify') }}</Button>
          <div class="flex flex-wrap justify-between gap-2 text-sm">
            <button
              type="button"
              class="text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
              @click="startOver"
            >
              {{ t('login.startOver') }}
            </button>
            <button
              type="button"
              class="text-primary underline-offset-4 hover:underline"
              @click="toggleRecovery"
            >
              {{ recovery ? t('login.useApp') : t('login.useRecovery') }}
            </button>
          </div>
        </form>
      </CardContent>
    </Card>
  </AuthFrame>
</template>
