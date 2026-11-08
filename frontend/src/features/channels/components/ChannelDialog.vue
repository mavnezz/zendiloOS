<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { ChannelInput, NotificationChannel } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useAccounts, useChannelActions } from '../api'

/** The channel to change; null adds one. */
const props = defineProps<{ channel: NotificationChannel | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t, locale } = useI18n()
const actions = useChannelActions()
const { data: accounts, error: accountsError } = useAccounts()

type Kind = 'email' | 'webhook'
type Format = 'json' | 'text'
type Language = 'en' | 'de'

interface Form {
  name: string
  kind: Kind
  /** The accounts mailed, by id. */
  accounts: number[]
  /** Other addresses, separated by commas or lines. */
  to: string
  url: string
  format: Format
  language: Language
}

function blank(): Form {
  return {
    name: '',
    kind: 'email',
    accounts: [],
    to: '',
    url: '',
    format: 'json',
    language: locale.value === 'de' ? 'de' : 'en',
  }
}

const form = ref<Form>(blank())
const failure = ref<string | null>(null)
const saving = ref(false)

watch(open, (isOpen) => {
  if (!isOpen) return
  failure.value = null
  const channel = props.channel
  const next = blank()
  if (channel) {
    next.name = channel.name
    next.language = channel.language
    const target = channel.target
    next.kind = target.kind
    if (target.kind === 'email') {
      next.accounts = [...(target.accounts ?? [])]
      next.to = (target.to ?? []).join(', ')
    } else {
      next.url = target.url
      next.format = target.format
    }
  }
  form.value = next
})

interface Choice {
  id: number
  name: string
  /** What the account is mailed at, or why it is not. */
  detail: string
}

/** The accounts to choose from; one chosen before and deleted since stays, to be taken out. */
const choices = computed<Choice[]>(() => {
  const listed: Choice[] = (accounts.value ?? []).map((account) => ({
    id: account.id,
    name: account.full_name ? `${account.full_name} (${account.name})` : account.name,
    detail: account.disabled
      ? t('channels.problems.disabled')
      : (account.email ?? t('channels.problems.no_address')),
  }))
  const gone = form.value.accounts
    .filter((id) => !listed.some((choice) => choice.id === id))
    .map((id) => ({
      id,
      name: t('channels.account', { id }),
      detail: accounts.value ? t('channels.problems.gone') : '',
    }))
  return [...listed, ...gone]
})

function addresses(): string[] {
  return form.value.to
    .split(/[\s,;]+/)
    .map((address) => address.trim())
    .filter(Boolean)
}

function body(): ChannelInput {
  const f = form.value
  return {
    name: f.name.trim(),
    target:
      f.kind === 'email'
        ? { kind: 'email', accounts: f.accounts, to: addresses() }
        : { kind: 'webhook', url: f.url.trim(), format: f.format },
    language: f.language,
  }
}

async function save(): Promise<void> {
  failure.value = null
  if (form.value.kind === 'email' && !form.value.accounts.length && !addresses().length) {
    failure.value = t('channels.noRecipient')
    return
  }
  saving.value = true
  try {
    if (props.channel) await actions.update(props.channel.id, body())
    else await actions.create(body())
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}

const select = 'h-9 w-full rounded-md border bg-background px-3 text-sm'
</script>

<!-- Adds or changes a channel: email to accounts and addresses, or a webhook. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="channel ? t('channels.edit', { name: channel.name }) : t('channels.add')"
    :description="t('channels.dialogHint')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-3" @submit.prevent="save">
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      <div class="grid gap-1.5">
        <Label for="channel-name">{{ t('channels.name') }}</Label>
        <Input id="channel-name" v-model="form.name" maxlength="64" required />
      </div>
      <div class="grid gap-1.5">
        <Label for="channel-kind">{{ t('channels.kind') }}</Label>
        <select id="channel-kind" v-model="form.kind" :class="select">
          <option value="email">{{ t('channels.kinds.email') }}</option>
          <option value="webhook">{{ t('channels.kinds.webhook') }}</option>
        </select>
      </div>

      <template v-if="form.kind === 'email'">
        <fieldset class="grid gap-1.5">
          <legend class="mb-1.5 text-sm font-medium">{{ t('channels.accounts') }}</legend>
          <p v-if="accountsError" class="text-xs text-muted-foreground">
            {{ t('channels.accountsUnknown') }}
          </p>
          <div
            v-if="choices.length"
            class="grid max-h-48 gap-1 overflow-y-auto rounded-md border p-2"
          >
            <label
              v-for="choice in choices"
              :key="choice.id"
              class="flex items-center gap-2 rounded px-1 py-0.5 text-sm hover:bg-muted/40"
            >
              <input
                v-model="form.accounts"
                type="checkbox"
                :value="choice.id"
                class="size-4 accent-primary"
              />
              <span class="truncate">{{ choice.name }}</span>
              <span class="ml-auto truncate text-xs text-muted-foreground">{{
                choice.detail
              }}</span>
            </label>
          </div>
          <p class="text-xs text-muted-foreground">{{ t('channels.accountsHint') }}</p>
        </fieldset>
        <div class="grid gap-1.5">
          <Label for="channel-to">{{ t('channels.to') }}</Label>
          <Input id="channel-to" v-model="form.to" placeholder="name@example.org" />
          <p class="text-xs text-muted-foreground">{{ t('channels.toHint') }}</p>
        </div>
      </template>

      <template v-else>
        <div class="grid gap-1.5">
          <Label for="channel-url">{{ t('channels.url') }}</Label>
          <Input id="channel-url" v-model="form.url" type="url" placeholder="https://" required />
        </div>
        <div class="grid gap-1.5">
          <Label for="channel-format">{{ t('channels.format') }}</Label>
          <select id="channel-format" v-model="form.format" :class="select">
            <option value="json">{{ t('channels.formats.json') }}</option>
            <option value="text">{{ t('channels.formats.text') }}</option>
          </select>
        </div>
      </template>

      <div class="grid gap-1.5">
        <Label for="channel-language">{{ t('channels.language') }}</Label>
        <select id="channel-language" v-model="form.language" :class="select">
          <option value="en">English</option>
          <option value="de">Deutsch</option>
        </select>
      </div>

      <div class="flex justify-end">
        <Button type="submit" :disabled="saving">{{ t('channels.save') }}</Button>
      </div>
    </form>
  </ModalDialog>
</template>
