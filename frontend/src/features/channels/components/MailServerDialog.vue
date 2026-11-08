<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { MailServerInput, MailServerSettings } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useChannelActions } from '../api'

/** The mail server as stored; null sets one up. */
const props = defineProps<{ server: MailServerSettings | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useChannelActions()

type Security = 'starttls' | 'tls' | 'none'

interface Form {
  server: string
  /** As typed. */
  port: string
  security: Security
  username: string
  /** Empty when one is stored: it stays. */
  password: string
  from: string
}

const form = ref<Form>({
  server: '',
  port: '587',
  security: 'starttls',
  username: '',
  password: '',
  from: '',
})
const failure = ref<string | null>(null)
const saving = ref(false)
const clearPassword = ref(false)

watch(open, (isOpen) => {
  if (!isOpen) return
  failure.value = null
  clearPassword.value = false
  const stored = props.server
  form.value = {
    server: stored?.server ?? '',
    port: String(stored?.port ?? 587),
    security: stored?.security ?? 'starttls',
    username: stored?.username ?? '',
    password: '',
    from: stored?.from ?? '',
  }
})

/** Typical ports follow the encryption, unless one was typed. */
watch(
  () => form.value.security,
  (security, before) => {
    const typical: Record<Security, string> = { starttls: '587', tls: '465', none: '25' }
    if (form.value.port === typical[before]) form.value.port = typical[security]
  },
)

function body(): MailServerInput {
  const f = form.value
  const input: MailServerInput = {
    server: f.server.trim(),
    port: Number(f.port),
    security: f.security,
    username: f.username.trim() || null,
    from: f.from.trim(),
  }
  // Left out, a stored password stays; empty, it goes.
  if (f.password || clearPassword.value) input.password = clearPassword.value ? '' : f.password
  return input
}

async function save(): Promise<void> {
  failure.value = null
  saving.value = true
  try {
    await actions.saveMailServer(body())
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}

const select = 'h-9 w-full rounded-md border bg-background px-3 text-sm'
</script>

<!-- Sets up the mail server every email channel sends through. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('channels.mailServer.title')"
    :description="t('channels.mailServer.dialogHint')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-3" @submit.prevent="save">
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      <div class="grid grid-cols-[1fr_6rem] gap-2">
        <div class="grid gap-1.5">
          <Label for="mail-server">{{ t('channels.mailServer.server') }}</Label>
          <Input id="mail-server" v-model="form.server" placeholder="smtp.example.org" required />
        </div>
        <div class="grid gap-1.5">
          <Label for="mail-port">{{ t('channels.mailServer.port') }}</Label>
          <Input id="mail-port" v-model="form.port" type="number" min="1" max="65535" required />
        </div>
      </div>
      <div class="grid gap-1.5">
        <Label for="mail-security">{{ t('channels.mailServer.security') }}</Label>
        <select id="mail-security" v-model="form.security" :class="select">
          <option value="starttls">{{ t('channels.mailServer.securities.starttls') }}</option>
          <option value="tls">{{ t('channels.mailServer.securities.tls') }}</option>
          <option value="none">{{ t('channels.mailServer.securities.none') }}</option>
        </select>
      </div>
      <div class="grid grid-cols-2 gap-2">
        <div class="grid gap-1.5">
          <Label for="mail-username">{{ t('channels.mailServer.username') }}</Label>
          <Input id="mail-username" v-model="form.username" autocomplete="off" />
        </div>
        <div class="grid gap-1.5">
          <Label for="mail-password">{{ t('channels.mailServer.password') }}</Label>
          <Input
            id="mail-password"
            v-model="form.password"
            type="password"
            autocomplete="new-password"
            :disabled="clearPassword"
            :placeholder="server?.password_set ? t('channels.mailServer.passwordKept') : ''"
          />
        </div>
      </div>
      <label v-if="server?.password_set" class="flex items-center gap-2 text-sm">
        <input v-model="clearPassword" type="checkbox" class="size-4 accent-primary" />
        {{ t('channels.mailServer.passwordClear') }}
      </label>
      <div class="grid gap-1.5">
        <Label for="mail-from">{{ t('channels.mailServer.from') }}</Label>
        <Input id="mail-from" v-model="form.from" type="email" required />
      </div>
      <div class="flex justify-end">
        <Button type="submit" :disabled="saving">{{ t('channels.save') }}</Button>
      </div>
    </form>
  </ModalDialog>
</template>
