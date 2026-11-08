<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AcmeAccount, AcmeChallenge, Certificate, CertificateKey } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { Switch } from '@/shared/ui/switch'

import { useAcmeActions } from '../acme'

/** The certificate to change; none: a new one. A change applies at its next issuance. */
const props = defineProps<{
  certificate: Certificate | null
  accounts: AcmeAccount[]
  challenges: AcmeChallenge[]
}>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ saved: [certificate: Certificate | null] }>()

const { t } = useI18n()
const actions = useAcmeActions()
const names = ref('')
const key = ref<CertificateKey>('ec256')
const account = ref(0)
const challenge = ref(0)
const autoRenew = ref(true)
const busy = ref(false)
const failure = ref<string | null>(null)

watch(open, (isOpen) => {
  if (!isOpen) return
  const certificate = props.certificate
  names.value = certificate?.domains.join('\n') ?? ''
  key.value = certificate?.key ?? 'ec256'
  account.value = certificate?.account_id ?? props.accounts[0]?.id ?? 0
  challenge.value = certificate?.challenge_id ?? props.challenges[0]?.id ?? 0
  autoRenew.value = certificate?.auto_renew ?? true
  failure.value = null
})

const domains = computed(() =>
  names.value
    .split(/[\s,]+/)
    .map((name) => name.trim().toLowerCase())
    .filter(Boolean),
)
const label = (c: AcmeChallenge) => `${c.name} (${t(`appStore.acme.methods.${c.method}`)})`

async function save(): Promise<void> {
  busy.value = true
  failure.value = null
  const body = {
    domains: domains.value,
    key: key.value,
    account_id: account.value,
    challenge_id: challenge.value,
    auto_renew: autoRenew.value,
  }
  try {
    if (props.certificate) {
      await actions.updateCertificate(props.certificate.id, body)
      emit('saved', null)
    } else {
      emit('saved', await actions.createCertificate(body))
    }
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}
</script>

<!-- A certificate's names, key, account and challenge. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('appStore.acme.certificate.title')"
    :close-label="t('desktop.close')"
  >
    <form id="acme-certificate" class="grid gap-4" @submit.prevent="save">
      <div class="grid gap-1.5">
        <Label for="acme-certificate-names">{{ t('appStore.acme.certificate.names') }}</Label>
        <textarea
          id="acme-certificate-names"
          v-model="names"
          rows="3"
          spellcheck="false"
          placeholder="nas.example.org"
          class="w-full rounded-md border border-input bg-transparent px-2.5 py-1.5 font-mono text-[13px] shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 dark:bg-input/30"
        />
        <p class="text-xs text-muted-foreground">{{ t('appStore.acme.certificate.namesHint') }}</p>
      </div>
      <div class="grid gap-1.5">
        <Label for="acme-certificate-account">{{ t('appStore.acme.certificate.account') }}</Label>
        <NativeSelect id="acme-certificate-account" v-model="account">
          <option v-for="a in accounts" :key="a.id" :value="a.id">
            {{ a.name }} · {{ t(`appStore.acme.servers.${a.server}`) }}
          </option>
        </NativeSelect>
      </div>
      <div class="grid gap-1.5">
        <Label for="acme-certificate-challenge">{{
          t('appStore.acme.certificate.challenge')
        }}</Label>
        <NativeSelect id="acme-certificate-challenge" v-model="challenge">
          <option v-for="c in challenges" :key="c.id" :value="c.id">{{ label(c) }}</option>
        </NativeSelect>
      </div>
      <div class="grid gap-1.5">
        <Label for="acme-certificate-key">{{ t('appStore.acme.certificate.key') }}</Label>
        <NativeSelect id="acme-certificate-key" v-model="key">
          <option value="ec256">{{ t('appStore.acme.keys.ec256') }}</option>
          <option value="rsa2048">{{ t('appStore.acme.keys.rsa2048') }}</option>
          <option value="rsa4096">{{ t('appStore.acme.keys.rsa4096') }}</option>
        </NativeSelect>
      </div>
      <label class="flex items-center gap-3 text-sm">
        <Switch v-model="autoRenew" />
        {{ t('appStore.acme.certificate.autoRenew') }}
      </label>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </form>
    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button
        type="submit"
        form="acme-certificate"
        :disabled="busy || !domains.length || !account || !challenge"
        >{{ t('common.apply') }}</Button
      >
    </template>
  </ModalDialog>
</template>
