<script setup lang="ts">
import { Copy, Download, ShieldCheck, ShieldOff } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { ApiProblem } from '@/shared/api/client'
import type { TwoFactorSetup, TwoFactorStatus } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { QrCode } from '@/shared/ui/qr-code'

import {
  beginTwoFactor,
  confirmTwoFactor,
  disableTwoFactor,
  fetchTwoFactor,
  renewRecoveryCodes,
} from '../api'
import { useSessionStore } from '../store'

const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const session = useSessionStore()
const notifications = useNotifications()

/**
 * `status`: on or off, with what can be done; `password`: the password, to
 * begin setting it up; `setup`: the QR code and the first code; `codes`:
 * recovery codes, shown once; `renew` and `disable`: their forms.
 */
type Step = 'status' | 'password' | 'setup' | 'codes' | 'renew' | 'disable'

const step = ref<Step>('status')
const status = ref<TwoFactorStatus | null>(null)
const setup = ref<TwoFactorSetup | null>(null)
const recoveryCodes = ref<string[]>([])
const code = ref('')
const password = ref('')
const error = ref<string | null>(null)
const pending = ref(false)

const name = computed(() => session.info?.account.name ?? '')
/** The key in groups of four, as apps show it. */
const key = computed(() => setup.value?.secret.match(/.{1,4}/g)?.join(' ') ?? '')

async function load(): Promise<void> {
  error.value = null
  try {
    status.value = await fetchTwoFactor()
  } catch (failure) {
    error.value = problemMessage(failure)
  }
}

watch(
  open,
  (isOpen) => {
    if (!isOpen) return
    step.value = 'status'
    status.value = null
    setup.value = null
    recoveryCodes.value = []
    void load()
  },
  { immediate: true },
)

function go(next: Step): void {
  step.value = next
  code.value = ''
  password.value = ''
  error.value = null
}

/** Runs `request`, showing what went wrong in the dialog. */
async function attempt(request: () => Promise<void>): Promise<void> {
  pending.value = true
  error.value = null
  try {
    await request()
  } catch (failure) {
    error.value = problemMessage(failure)
    if (failure instanceof ApiProblem && failure.code === 'wrong_password') password.value = ''
    else code.value = ''
  } finally {
    pending.value = false
  }
}

const begin = () =>
  attempt(async () => {
    setup.value = await beginTwoFactor(password.value)
    go('setup')
  })

const confirm = () =>
  attempt(async () => {
    recoveryCodes.value = await confirmTwoFactor(code.value)
    setup.value = null
    go('codes')
    notifications.notify('success', t('twoFactor.turnedOn'), t('twoFactor.turnedOnHint'))
  })

const renew = () =>
  attempt(async () => {
    recoveryCodes.value = await renewRecoveryCodes(password.value, code.value)
    go('codes')
  })

const disable = () =>
  attempt(async () => {
    await disableTwoFactor(password.value, code.value)
    notifications.notify('success', t('twoFactor.turnedOff'))
    go('status')
    await load()
  })

async function finish(): Promise<void> {
  recoveryCodes.value = []
  go('status')
  await load()
}

async function copy(text: string, done: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
    notifications.notify('success', done)
  } catch (failure) {
    notifications.notify('error', problemMessage(failure))
  }
}

function download(): void {
  const text = `${t('twoFactor.file', { name: name.value })}\n\n${recoveryCodes.value.join('\n')}\n`
  const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }))
  const link = document.createElement('a')
  link.href = url
  link.download = `zendiloos-recovery-codes-${name.value}.txt`
  link.click()
  URL.revokeObjectURL(url)
}
</script>

<!-- The signed-in account's authenticator app: set it up, renew its recovery codes, turn it off. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('twoFactor.title')"
    :description="t('twoFactor.description')"
    :close-label="t('desktop.close')"
  >
    <Alert v-if="error" variant="destructive">{{ error }}</Alert>

    <template v-if="step === 'status' && status">
      <div class="flex items-start gap-3 rounded-lg border p-3">
        <component
          :is="status.enabled ? ShieldCheck : ShieldOff"
          class="mt-0.5 size-5 shrink-0"
          :class="
            status.enabled ? 'text-emerald-600 dark:text-emerald-400' : 'text-muted-foreground'
          "
        />
        <div class="grid gap-1 text-sm">
          <p class="flex items-center gap-2 font-medium">
            {{ t('twoFactor.title') }}
            <Badge :variant="status.enabled ? 'success' : 'outline'">
              {{ status.enabled ? t('twoFactor.on') : t('twoFactor.off') }}
            </Badge>
          </p>
          <p class="text-muted-foreground">
            {{ status.enabled ? t('twoFactor.onStatus') : t('twoFactor.offStatus') }}
          </p>
          <p v-if="status.enabled" class="text-muted-foreground" data-testid="codes-left">
            {{ t('twoFactor.codesLeft', status.recovery_codes_left) }}
          </p>
        </div>
      </div>
      <div class="flex flex-wrap justify-end gap-2">
        <template v-if="status.enabled">
          <Button variant="outline" @click="go('renew')">{{ t('twoFactor.renew') }}</Button>
          <Button variant="destructive" @click="go('disable')">
            {{ t('twoFactor.disable') }}
          </Button>
        </template>
        <Button v-else @click="go('password')">{{ t('twoFactor.setUp') }}</Button>
      </div>
    </template>

    <form v-else-if="step === 'password'" class="grid gap-4" @submit.prevent="begin">
      <p class="text-sm text-muted-foreground">{{ t('twoFactor.passwordHint') }}</p>
      <div class="grid gap-2">
        <Label for="two-factor-password">{{ t('twoFactor.password') }}</Label>
        <Input
          id="two-factor-password"
          v-model="password"
          type="password"
          autocomplete="current-password"
          required
        />
      </div>
      <div class="flex flex-wrap justify-end gap-2">
        <Button type="button" variant="outline" @click="go('status')">
          {{ t('twoFactor.back') }}
        </Button>
        <Button type="submit" :disabled="pending">{{ t('twoFactor.continue') }}</Button>
      </div>
    </form>

    <form v-else-if="step === 'setup' && setup" class="grid gap-4" @submit.prevent="confirm">
      <p class="text-sm text-muted-foreground">{{ t('twoFactor.scan') }}</p>
      <QrCode
        :value="setup.uri"
        :label="t('twoFactor.qrLabel')"
        class="size-48 justify-self-center"
      />
      <div class="grid gap-1">
        <p class="text-xs text-muted-foreground">{{ t('twoFactor.orType') }}</p>
        <div class="flex items-center gap-2">
          <code
            class="flex-1 rounded-md bg-muted px-2 py-1.5 font-mono text-sm break-all"
            data-testid="key"
          >
            {{ key }}
          </code>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            :aria-label="t('twoFactor.copyKey')"
            @click="copy(setup.secret, t('twoFactor.keyCopied'))"
          >
            <Copy />
          </Button>
        </div>
      </div>
      <div class="grid gap-2">
        <Label for="two-factor-code">{{ t('twoFactor.code') }}</Label>
        <Input
          id="two-factor-code"
          v-model="code"
          class="font-mono tracking-widest"
          inputmode="numeric"
          autocomplete="one-time-code"
          maxlength="6"
          required
        />
      </div>
      <div class="flex flex-wrap justify-end gap-2">
        <Button type="button" variant="outline" @click="go('status')">
          {{ t('twoFactor.back') }}
        </Button>
        <Button type="submit" :disabled="pending">{{ t('twoFactor.turnOn') }}</Button>
      </div>
    </form>

    <template v-else-if="step === 'codes'">
      <div class="grid gap-1">
        <p class="text-sm font-medium">{{ t('twoFactor.recoveryTitle') }}</p>
        <p class="text-sm text-muted-foreground">{{ t('twoFactor.recoveryHint') }}</p>
      </div>
      <ul
        class="grid grid-cols-2 gap-x-6 gap-y-1 rounded-lg bg-muted px-4 py-3 font-mono text-sm"
        data-testid="recovery-codes"
      >
        <li v-for="recoveryCode in recoveryCodes" :key="recoveryCode">{{ recoveryCode }}</li>
      </ul>
      <div class="flex flex-wrap justify-end gap-2">
        <Button variant="outline" @click="copy(recoveryCodes.join('\n'), t('twoFactor.copied'))">
          <Copy />
          {{ t('twoFactor.copy') }}
        </Button>
        <Button variant="outline" @click="download">
          <Download />
          {{ t('twoFactor.download') }}
        </Button>
        <Button @click="finish">{{ t('twoFactor.done') }}</Button>
      </div>
    </template>

    <form v-else-if="step === 'renew'" class="grid gap-4" @submit.prevent="renew">
      <p class="text-sm text-muted-foreground">{{ t('twoFactor.renewHint') }}</p>
      <div class="grid gap-2">
        <Label for="two-factor-password">{{ t('twoFactor.password') }}</Label>
        <Input
          id="two-factor-password"
          v-model="password"
          type="password"
          autocomplete="current-password"
          required
        />
      </div>
      <div class="grid gap-2">
        <Label for="two-factor-code">{{ t('twoFactor.code') }}</Label>
        <Input
          id="two-factor-code"
          v-model="code"
          class="font-mono tracking-widest"
          inputmode="numeric"
          autocomplete="one-time-code"
          maxlength="6"
          required
        />
      </div>
      <div class="flex flex-wrap justify-end gap-2">
        <Button type="button" variant="outline" @click="go('status')">
          {{ t('twoFactor.back') }}
        </Button>
        <Button type="submit" :disabled="pending">{{ t('twoFactor.renewAction') }}</Button>
      </div>
    </form>

    <form v-else-if="step === 'disable'" class="grid gap-4" @submit.prevent="disable">
      <p class="text-sm text-muted-foreground">{{ t('twoFactor.disableHint') }}</p>
      <div class="grid gap-2">
        <Label for="two-factor-password">{{ t('twoFactor.password') }}</Label>
        <Input
          id="two-factor-password"
          v-model="password"
          type="password"
          autocomplete="current-password"
          required
        />
      </div>
      <div class="grid gap-2">
        <Label for="two-factor-code">{{ t('twoFactor.codeOrRecovery') }}</Label>
        <Input
          id="two-factor-code"
          v-model="code"
          class="font-mono"
          autocomplete="one-time-code"
          autocapitalize="none"
          spellcheck="false"
          required
        />
      </div>
      <div class="flex flex-wrap justify-end gap-2">
        <Button type="button" variant="outline" @click="go('status')">
          {{ t('twoFactor.back') }}
        </Button>
        <Button type="submit" variant="destructive" :disabled="pending">
          {{ t('twoFactor.disableAction') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
