<script setup lang="ts">
import { Mail, Pencil, Plus, Send, Server, Trash2, Webhook } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { ChannelRecipient, NotificationChannel } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { PanelSection } from '@/shared/ui/section'

import { useAccounts, useChannelActions, useChannels, useMailServer } from '../api'
import ChannelDialog from './ChannelDialog.vue'
import MailServerDialog from './MailServerDialog.vue'
import RoutesTable from './RoutesTable.vue'

const { t } = useI18n()
const { data: channels, error } = useChannels()
const { data: server, error: serverError } = useMailServer()
const { data: accounts } = useAccounts()
const actions = useChannelActions()

const editing = ref<NotificationChannel | null>(null)
const dialogOpen = ref(false)
const serverOpen = ref(false)
const removing = ref<NotificationChannel | null>(null)
const removeOpen = ref(false)
/** The outcome of the last test, per channel. */
const tests = ref<Record<number, { ok: boolean; text: string } | 'sending'>>({})
const failure = ref<string | null>(null)

const mailing = computed(() =>
  (channels.value ?? []).some((channel) => channel.target.kind === 'email'),
)

function add(): void {
  editing.value = null
  dialogOpen.value = true
}

function edit(channel: NotificationChannel): void {
  editing.value = channel
  dialogOpen.value = true
}

function host(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

/** A recipient as people know it: the account's name, and where mail to it goes. */
function recipient(entry: ChannelRecipient): { name: string; detail: string | null } {
  if (entry.account_id === null || entry.account_id === undefined) {
    return { name: entry.address ?? '', detail: null }
  }
  const id = entry.account_id
  const account = accounts.value?.find((candidate) => candidate.id === id)
  const name = account?.name ?? t('channels.account', { id })
  if (entry.problem) return { name, detail: t(`channels.problems.${entry.problem}`) }
  return { name, detail: entry.address ?? null }
}

async function test(channel: NotificationChannel): Promise<void> {
  tests.value = { ...tests.value, [channel.id]: 'sending' }
  try {
    await actions.test(channel.id)
    tests.value = { ...tests.value, [channel.id]: { ok: true, text: t('channels.testSent') } }
  } catch (problem) {
    tests.value = { ...tests.value, [channel.id]: { ok: false, text: problemMessage(problem) } }
  }
}

async function remove(): Promise<void> {
  const channel = removing.value
  if (!channel) return
  failure.value = null
  try {
    await actions.remove(channel.id)
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}
</script>

<!-- Where notifications go besides the interface: the mail server, the
     channels, and which kinds go where. -->
<template>
  <div class="grid gap-8">
    <p class="text-sm text-muted-foreground">{{ t('channels.hint') }}</p>

    <PanelSection :title="t('channels.mailServer.title')">
      <Alert v-if="serverError" variant="destructive">{{ problemMessage(serverError) }}</Alert>
      <div class="flex items-center gap-3 rounded-xl border bg-card p-4">
        <Server class="size-4 shrink-0 text-primary" />
        <div class="grid min-w-0 flex-1 gap-0.5">
          <template v-if="server">
            <span class="truncate font-medium">{{ server.server }}:{{ server.port }}</span>
            <span class="truncate text-sm text-muted-foreground">
              {{ t(`channels.mailServer.short.${server.security}`) }} · {{ server.from }}
            </span>
          </template>
          <span
            v-else
            class="text-sm"
            :class="mailing ? 'text-destructive' : 'text-muted-foreground'"
          >
            {{ t('channels.mailServer.none') }}
          </span>
        </div>
        <Button size="sm" variant="outline" @click="serverOpen = true">
          <Pencil />
          {{ server ? t('channels.change') : t('channels.mailServer.setUp') }}
        </Button>
      </div>
    </PanelSection>

    <PanelSection :title="t('channels.title')">
      <template #actions>
        <Button size="sm" @click="add">
          <Plus />
          {{ t('channels.add') }}
        </Button>
      </template>
      <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      <p v-if="channels && !channels.length" class="text-sm text-muted-foreground">
        {{ t('channels.none') }}
      </p>

      <ul class="grid gap-3">
        <li
          v-for="channel in channels"
          :key="channel.id"
          class="grid gap-2 rounded-xl border bg-card p-4"
        >
          <div class="flex items-center gap-2">
            <component
              :is="channel.target.kind === 'email' ? Mail : Webhook"
              class="size-4 shrink-0 text-primary"
            />
            <span class="min-w-0 flex-1 truncate font-medium">{{ channel.name }}</span>
          </div>
          <ul
            v-if="channel.target.kind === 'email'"
            class="flex flex-wrap gap-x-4 gap-y-1 text-sm"
            :aria-label="t('channels.recipients')"
          >
            <li v-for="(entry, index) in channel.recipients" :key="index" class="min-w-0 truncate">
              {{ recipient(entry).name }}
              <span
                v-if="recipient(entry).detail"
                :class="entry.problem ? 'text-destructive' : 'text-muted-foreground'"
              >
                ({{ recipient(entry).detail }})
              </span>
            </li>
          </ul>
          <p v-else class="truncate text-sm text-muted-foreground">
            {{ host(channel.target.url) }}
          </p>
          <p v-if="channel.last_delivery_at" class="text-xs text-muted-foreground">
            <template v-if="channel.last_error">
              <span class="text-destructive">
                {{
                  t('channels.lastFailed', {
                    time: formatDateTime(channel.last_delivery_at * 1000),
                  })
                }}
              </span>
              {{ channel.last_error }}
            </template>
            <template v-else>
              {{
                t('channels.lastSent', { time: formatDateTime(channel.last_delivery_at * 1000) })
              }}
            </template>
          </p>
          <p
            v-if="tests[channel.id] && tests[channel.id] !== 'sending'"
            class="text-sm"
            :class="
              (tests[channel.id] as { ok: boolean }).ok
                ? 'text-emerald-700 dark:text-emerald-400'
                : 'text-destructive'
            "
            role="status"
          >
            {{ (tests[channel.id] as { text: string }).text }}
          </p>
          <div class="flex flex-wrap justify-end gap-2">
            <Button
              size="sm"
              variant="outline"
              :disabled="tests[channel.id] === 'sending'"
              @click="test(channel)"
            >
              <Send />
              {{ t('channels.test') }}
            </Button>
            <Button size="sm" variant="ghost" @click="edit(channel)">
              <Pencil />
              {{ t('channels.change') }}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              :aria-label="t('channels.remove')"
              :title="t('channels.remove')"
              @click="((removing = channel), (removeOpen = true))"
            >
              <Trash2 />
            </Button>
          </div>
        </li>
      </ul>
    </PanelSection>

    <PanelSection :title="t('channels.routes.title')" :description="t('channels.routes.hint')">
      <RoutesTable :channels="channels ?? []" />
    </PanelSection>

    <MailServerDialog v-model:open="serverOpen" :server="server ?? null" />
    <ChannelDialog v-model:open="dialogOpen" :channel="editing" />
    <ConfirmDialog
      v-model:open="removeOpen"
      :title="t('channels.removeTitle', { name: removing?.name ?? '' })"
      :description="t('channels.removeHint')"
      :confirm-label="t('channels.remove')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="remove"
    />
  </div>
</template>
