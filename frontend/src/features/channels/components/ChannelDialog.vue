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

import { useChannelActions } from '../api'

/** The channel to change; null adds one. */
const props = defineProps<{ channel: NotificationChannel | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t, locale } = useI18n()
const actions = useChannelActions()

type Kind = 'email' | 'webhook'
type Security = 'starttls' | 'tls' | 'none'
type Format = 'json' | 'text'
type Level = 'info' | 'warning' | 'error'
type Language = 'en' | 'de'

interface Form {
  name: string
  kind: Kind
  server: string
  /** As typed. */
  port: string
  security: Security
  username: string
  /** Empty when changing a channel: the stored one stays. */
  password: string
  from: string
  /** Addresses, separated by commas or lines. */
  to: string
  url: string
  format: Format
  minLevel: Level
  language: Language
}

function blank(): Form {
  return {
    name: '',
    kind: 'email',
    server: '',
    port: '587',
    security: 'starttls',
    username: '',
    password: '',
    from: '',
    to: '',
    url: '',
    format: 'json',
    minLevel: 'warning',
    language: locale.value === 'de' ? 'de' : 'en',
  }
}

const form = ref<Form>(blank())
const failure = ref<string | null>(null)
const saving = ref(false)
const clearPassword = ref(false)

watch(open, (isOpen) => {
  if (!isOpen) return
  failure.value = null
  clearPassword.value = false
  const channel = props.channel
  const next = blank()
  if (channel) {
    next.name = channel.name
    next.minLevel = channel.min_level
    next.language = channel.language
    const target = channel.target
    next.kind = target.kind
    if (target.kind === 'email') {
      next.server = target.server
      next.port = String(target.port)
      next.security = target.security
      next.username = target.username ?? ''
      next.from = target.from
      next.to = target.to.join(', ')
    } else {
      next.url = target.url
      next.format = target.format
    }
  }
  form.value = next
})

const passwordStored = computed(
  () => props.channel?.target.kind === 'email' && props.channel.password_set,
)

/** Typical ports follow the encryption, unless one was typed. */
watch(
  () => form.value.security,
  (security, before) => {
    const typical: Record<Security, string> = { starttls: '587', tls: '465', none: '25' }
    if (form.value.port === typical[before]) form.value.port = typical[security]
  },
)

function body(): ChannelInput {
  const f = form.value
  const target: ChannelInput['target'] =
    f.kind === 'email'
      ? {
          kind: 'email',
          server: f.server,
          port: Number(f.port),
          security: f.security,
          username: f.username || null,
          from: f.from,
          to: f.to
            .split(/[\s,;]+/)
            .map((address) => address.trim())
            .filter(Boolean),
        }
      : { kind: 'webhook', url: f.url, format: f.format }
  const input: ChannelInput = {
    name: f.name,
    target,
    min_level: f.minLevel,
    language: f.language,
  }
  // Left out, a stored password stays; empty, it goes.
  if (f.kind === 'email' && (f.password || clearPassword.value || !props.channel)) {
    input.password = clearPassword.value ? '' : f.password
  }
  return input
}

async function save(): Promise<void> {
  failure.value = null
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

<!-- Adds or changes a channel: email over SMTP, or a webhook. -->
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
        <div class="grid grid-cols-[1fr_6rem] gap-2">
          <div class="grid gap-1.5">
            <Label for="channel-server">{{ t('channels.server') }}</Label>
            <Input
              id="channel-server"
              v-model="form.server"
              placeholder="smtp.example.org"
              required
            />
          </div>
          <div class="grid gap-1.5">
            <Label for="channel-port">{{ t('channels.port') }}</Label>
            <Input
              id="channel-port"
              v-model="form.port"
              type="number"
              min="1"
              max="65535"
              required
            />
          </div>
        </div>
        <div class="grid gap-1.5">
          <Label for="channel-security">{{ t('channels.security') }}</Label>
          <select id="channel-security" v-model="form.security" :class="select">
            <option value="starttls">{{ t('channels.securities.starttls') }}</option>
            <option value="tls">{{ t('channels.securities.tls') }}</option>
            <option value="none">{{ t('channels.securities.none') }}</option>
          </select>
        </div>
        <div class="grid grid-cols-2 gap-2">
          <div class="grid gap-1.5">
            <Label for="channel-username">{{ t('channels.username') }}</Label>
            <Input id="channel-username" v-model="form.username" autocomplete="off" />
          </div>
          <div class="grid gap-1.5">
            <Label for="channel-password">{{ t('channels.password') }}</Label>
            <Input
              id="channel-password"
              v-model="form.password"
              type="password"
              autocomplete="new-password"
              :disabled="clearPassword"
              :placeholder="passwordStored ? t('channels.passwordKept') : ''"
            />
          </div>
        </div>
        <label v-if="passwordStored" class="flex items-center gap-2 text-sm">
          <input v-model="clearPassword" type="checkbox" class="size-4 accent-primary" />
          {{ t('channels.passwordClear') }}
        </label>
        <div class="grid gap-1.5">
          <Label for="channel-from">{{ t('channels.from') }}</Label>
          <Input id="channel-from" v-model="form.from" type="email" required />
        </div>
        <div class="grid gap-1.5">
          <Label for="channel-to">{{ t('channels.to') }}</Label>
          <Input id="channel-to" v-model="form.to" required />
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

      <div class="grid grid-cols-2 gap-2">
        <div class="grid gap-1.5">
          <Label for="channel-level">{{ t('channels.minLevel') }}</Label>
          <select id="channel-level" v-model="form.minLevel" :class="select">
            <option value="info">{{ t('channels.levels.info') }}</option>
            <option value="warning">{{ t('channels.levels.warning') }}</option>
            <option value="error">{{ t('channels.levels.error') }}</option>
          </select>
        </div>
        <div class="grid gap-1.5">
          <Label for="channel-language">{{ t('channels.language') }}</Label>
          <select id="channel-language" v-model="form.language" :class="select">
            <option value="en">English</option>
            <option value="de">Deutsch</option>
          </select>
        </div>
      </div>

      <div class="flex justify-end">
        <Button type="submit" :disabled="saving">{{ t('channels.save') }}</Button>
      </div>
    </form>
  </ModalDialog>
</template>
