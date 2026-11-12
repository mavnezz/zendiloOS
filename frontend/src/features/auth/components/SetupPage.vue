<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { ApiProblem } from '@/shared/api/client'
import type { RestoreResult } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { waitForServer } from '@/shared/lib/passphrase'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/shared/ui/card'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { completeSetup, fetchSetupStatus, restoreSetup } from '../api'
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
/** Creating the first administrator, or restoring a configuration backup. */
const mode = ref<'create' | 'restore'>('create')
const backup = ref<File | null>(null)
const passphrase = ref('')
const restored = ref<RestoreResult | null>(null)
/** Back after the restart, or not within the time waited. */
const back = ref<boolean | null>(null)

/**
 * Whether the setup asks for the code on the console. An open setup (a NAS
 * installed from the ISO) does not; unknown until asked, and asked for if
 * the answer does not come.
 */
const codeRequired = ref<boolean | null>(null)
onMounted(async () => {
  try {
    codeRequired.value = (await fetchSetupStatus()).code_required
  } catch {
    codeRequired.value = true
  }
})

const passwordsDiffer = computed(
  () => passwordRepeat.value !== '' && password.value !== passwordRepeat.value,
)

function chosen(event: Event): void {
  backup.value = (event.target as HTMLInputElement).files?.[0] ?? null
}

async function restore(): Promise<void> {
  if (!backup.value) return
  pending.value = true
  error.value = null
  try {
    const code = codeRequired.value === false ? null : setupCode.value
    restored.value = await restoreSetup(backup.value, passphrase.value, code)
    passphrase.value = ''
    back.value = await waitForServer()
    if (back.value) await router.push({ name: 'login' })
  } catch (failure) {
    error.value = problemMessage(failure)
    completedElsewhere.value = failure instanceof ApiProblem && failure.code === 'setup_completed'
  } finally {
    pending.value = false
  }
}

async function submit(): Promise<void> {
  if (mode.value === 'restore') return restore()
  if (password.value !== passwordRepeat.value) return
  pending.value = true
  error.value = null
  try {
    session.start(
      await completeSetup({
        ...(codeRequired.value === false ? {} : { setup_code: setupCode.value }),
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
        <div v-if="restored" class="grid gap-3 text-sm" data-testid="restored">
          <p class="font-medium">
            {{
              t('setup.restored', {
                name: restored.host_name,
                time: formatDateTime(restored.created_at * 1000),
              })
            }}
          </p>
          <ul v-if="restored.warnings.length" class="list-disc pl-5 text-muted-foreground">
            <li v-for="warning in restored.warnings" :key="warning">{{ warning }}</li>
          </ul>
          <p class="text-muted-foreground">
            {{ back === false ? t('setup.restoredSlow') : t('setup.restarting') }}
          </p>
          <RouterLink v-if="back === false" :to="{ name: 'login' }" class="underline">
            {{ t('setup.toLogin') }}
          </RouterLink>
        </div>
        <form v-else class="grid gap-5" @submit.prevent="submit">
          <Alert v-if="error" variant="destructive" class="flex-col gap-2">
            <span>{{ error }}</span>
            <RouterLink v-if="completedElsewhere" :to="{ name: 'login' }" class="underline">
              {{ t('setup.toLogin') }}
            </RouterLink>
          </Alert>
          <p v-if="codeRequired === false" class="text-sm text-muted-foreground">
            {{ t('setup.openHint') }}
          </p>
          <div v-else-if="codeRequired" class="grid gap-2">
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
          <template v-if="mode === 'restore'">
            <p class="text-sm text-muted-foreground">{{ t('setup.restoreIntro') }}</p>
            <div class="grid gap-2">
              <Label for="setup-backup">{{ t('setup.backupFile') }}</Label>
              <Input
                id="setup-backup"
                type="file"
                accept=".age,application/octet-stream"
                @change="chosen"
              />
            </div>
            <div class="grid gap-2">
              <Label for="setup-passphrase">{{ t('setup.passphrase') }}</Label>
              <Input
                id="setup-passphrase"
                v-model="passphrase"
                type="password"
                autocomplete="off"
                required
              />
            </div>
            <Button type="submit" class="w-full" :disabled="pending || !backup || !passphrase">
              {{ pending ? t('setup.restoring') : t('setup.restore') }}
            </Button>
            <Button type="button" variant="link" @click="mode = 'create'">
              {{ t('setup.createInstead') }}
            </Button>
          </template>
          <template v-else>
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
            <Button
              type="button"
              variant="link"
              data-testid="restore-instead"
              @click="mode = 'restore'"
            >
              {{ t('setup.restoreInstead') }}
            </Button>
          </template>
        </form>
      </CardContent>
    </Card>
  </AuthFrame>
</template>
