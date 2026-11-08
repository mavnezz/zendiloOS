<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AcmeAccount, AcmeServerName } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'

import { useAcmeActions } from '../acme'

/** The account to change; none: a new one. */
const props = defineProps<{ account: AcmeAccount | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useAcmeActions()
const name = ref('')
const server = ref<AcmeServerName>('letsencrypt')
const email = ref('')
const busy = ref(false)
const failure = ref<string | null>(null)

watch(open, (isOpen) => {
  if (!isOpen) return
  name.value = props.account?.name ?? "Let's Encrypt"
  server.value = props.account?.server ?? 'letsencrypt'
  email.value = props.account?.email ?? ''
  failure.value = null
})

async function save(): Promise<void> {
  busy.value = true
  failure.value = null
  const body = { name: name.value.trim(), server: server.value, email: email.value.trim() }
  try {
    if (props.account) await actions.updateAccount(props.account.id, body)
    else await actions.createAccount(body)
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}
</script>

<!-- An account at Let's Encrypt (or its staging server for trying out). -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('appStore.acme.account.title')"
    :close-label="t('desktop.close')"
  >
    <form id="acme-account" class="grid gap-4" @submit.prevent="save">
      <div class="grid gap-1.5">
        <Label for="acme-account-name">{{ t('appStore.acme.account.name') }}</Label>
        <Input id="acme-account-name" v-model="name" required />
      </div>
      <div class="grid gap-1.5">
        <Label for="acme-account-server">{{ t('appStore.acme.account.server') }}</Label>
        <NativeSelect id="acme-account-server" v-model="server">
          <option value="letsencrypt">{{ t('appStore.acme.servers.letsencrypt') }}</option>
          <option value="letsencrypt_staging">
            {{ t('appStore.acme.servers.letsencrypt_staging') }}
          </option>
        </NativeSelect>
        <p v-if="server === 'letsencrypt_staging'" class="text-xs text-muted-foreground">
          {{ t('appStore.acme.account.stagingHint') }}
        </p>
      </div>
      <div class="grid gap-1.5">
        <Label for="acme-account-email">{{ t('appStore.acme.account.email') }}</Label>
        <Input id="acme-account-email" v-model="email" type="email" required />
        <p class="text-xs text-muted-foreground">{{ t('appStore.acme.account.emailHint') }}</p>
      </div>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </form>
    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button type="submit" form="acme-account" :disabled="busy || !name.trim() || !email.trim()">{{
        t('common.apply')
      }}</Button>
    </template>
  </ModalDialog>
</template>
