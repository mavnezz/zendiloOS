<script setup lang="ts">
import { ExternalLink } from 'lucide-vue-next'
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BackupCloudAccount, BackupProvider } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button, buttonVariants } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { Switch } from '@/shared/ui/switch'

import { useBackupActions } from '../api'

/**
 * Signs in to Google Drive or OneDrive without rclone on this computer:
 * the provider's consent page opens in a tab of its own, and the address
 * it sends the browser on to (which shows an error) is pasted back.
 */
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ signedIn: [account: BackupCloudAccount] }>()

const PROVIDERS: BackupProvider[] = ['google_drive', 'onedrive']

const { t } = useI18n()
const actions = useBackupActions()
const provider = ref<BackupProvider>('google_drive')
const name = ref('')
const own = ref(false)
const clientId = ref('')
const clientSecret = ref('')
const begun = ref<{ flow: string; url: string } | null>(null)
const address = ref('')
const pending = ref(false)
const error = ref<string | null>(null)

watch(
  open,
  (isOpen) => {
    if (!isOpen) return
    provider.value = 'google_drive'
    name.value = t('backup.accounts.providers.google_drive')
    own.value = false
    clientId.value = ''
    clientSecret.value = ''
    begun.value = null
    address.value = ''
    error.value = null
  },
  { immediate: true },
)

function choose(choice: BackupProvider): void {
  const named = PROVIDERS.some((p) => name.value === t(`backup.accounts.providers.${p}`))
  if (!name.value.trim() || named) name.value = t(`backup.accounts.providers.${choice}`)
  provider.value = choice
}

async function attempt(work: () => Promise<void>): Promise<void> {
  pending.value = true
  error.value = null
  try {
    await work()
  } catch (problem) {
    error.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}

const begin = () =>
  attempt(async () => {
    begun.value = await actions.startSignIn({
      provider: provider.value,
      name: name.value.trim(),
      client_id: own.value && clientId.value.trim() ? clientId.value.trim() : null,
      client_secret: own.value && clientSecret.value ? clientSecret.value : null,
    })
  })

const finish = () =>
  attempt(async () => {
    const account = await actions.finishSignIn(begun.value!.flow, address.value.trim())
    emit('signedIn', account)
    open.value = false
  })
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="t('backup.signIn.title')"
    :close-label="t('desktop.close')"
  >
    <form v-if="!begun" id="backup-sign-in" class="grid gap-4" @submit.prevent="begin">
      <div
        class="flex flex-wrap gap-4 text-sm"
        role="radiogroup"
        :aria-label="t('backup.signIn.provider')"
      >
        <label v-for="choice in PROVIDERS" :key="choice" class="flex items-center gap-2">
          <input
            type="radio"
            name="backup-provider"
            :checked="provider === choice"
            @change="choose(choice)"
          />
          {{ t(`backup.accounts.providers.${choice}`) }}
        </label>
      </div>
      <div class="grid gap-1.5">
        <Label for="backup-account-name">{{ t('backup.signIn.name') }}</Label>
        <Input id="backup-account-name" v-model="name" maxlength="64" />
      </div>
      <label class="flex items-center gap-3 text-sm">
        <Switch v-model="own" />
        {{ t('backup.signIn.own') }}
      </label>
      <template v-if="own">
        <div class="grid gap-1.5">
          <Label for="backup-client-id">{{ t('backup.signIn.clientId') }}</Label>
          <Input id="backup-client-id" v-model="clientId" autocomplete="off" />
        </div>
        <div class="grid gap-1.5">
          <Label for="backup-client-secret">{{ t('backup.signIn.clientSecret') }}</Label>
          <Input
            id="backup-client-secret"
            v-model="clientSecret"
            type="password"
            autocomplete="off"
          />
        </div>
        <p class="text-xs text-muted-foreground">{{ t('backup.signIn.ownHint') }}</p>
      </template>
      <p v-else class="text-xs text-muted-foreground">{{ t('backup.signIn.rcloneHint') }}</p>
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
    </form>
    <form v-else id="backup-sign-in" class="grid gap-4" @submit.prevent="finish">
      <ol class="grid list-decimal gap-2 pl-5 text-sm">
        <li>
          {{ t('backup.signIn.stepOpen') }}
          <a
            :href="begun.url"
            target="_blank"
            rel="noopener noreferrer"
            :class="buttonVariants({ variant: 'outline', size: 'sm' })"
            class="mt-2 w-fit"
            data-testid="backup-consent"
            ><ExternalLink /> {{ t('backup.signIn.open') }}</a
          >
        </li>
        <li>{{ t('backup.signIn.stepAgree') }}</li>
        <li>{{ t('backup.signIn.stepPaste') }}</li>
      </ol>
      <div class="grid gap-1.5">
        <Label for="backup-sign-in-address">{{ t('backup.signIn.address') }}</Label>
        <Input
          id="backup-sign-in-address"
          v-model="address"
          placeholder="http://127.0.0.1:53682/?state=…&code=…"
          autocomplete="off"
        />
      </div>
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
    </form>
    <template #footer>
      <Button v-if="begun" type="button" variant="secondary" @click="begun = null">{{
        t('backup.wizard.back')
      }}</Button>
      <Button
        type="submit"
        form="backup-sign-in"
        :disabled="pending || (begun ? !address.trim() : !name.trim())"
        >{{ begun ? t('backup.signIn.finish') : t('backup.signIn.begin') }}</Button
      >
    </template>
  </ModalDialog>
</template>
