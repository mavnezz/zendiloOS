<script setup lang="ts">
import { Mail, Pencil, Plus, Send, Server, Trash2, Webhook } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type {
  ChannelRecipient,
  NotificationChannel,
  UndeliveredNotification,
} from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { wording } from '@/shared/notices/store'
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

/** Where mail goes out, in one line: server and port, encryption, sender. */
const serverLine = computed(() => {
  const value = server.value
  if (!value) return ''
  const security = t(`channels.mailServer.short.${value.security}`)
  return `${value.server}:${value.port} · ${security} · ${value.from}`
})

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

/** A notification that has not gone out, by its title and when. */
function undelivered(item: UndeliveredNotification): string {
  return `${wording(item.notification).title} (${formatDateTime(item.notification.at * 1000)})`
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
    <div class="grid gap-3">
      <p class="text-sm text-muted-foreground">{{ t('channels.hint') }}</p>
      <!-- The mail server serves every email channel: one quiet line. -->
      <div
        class="flex flex-wrap items-center gap-2 rounded-lg bg-muted/50 px-3 py-2 text-xs"
        data-testid="mail-server"
      >
        <Server class="size-4 shrink-0 text-muted-foreground" />
        <p class="min-w-0 flex-1 truncate">
          <span class="font-medium">{{ t('channels.mailServer.title') }}:</span>
          {{ serverLine }}
          <span v-if="!server" :class="mailing ? 'text-destructive' : 'text-muted-foreground'">
            {{ t('channels.mailServer.none') }}
          </span>
        </p>
        <Button variant="soft" size="sm" shape="pill" @click="serverOpen = true">
          {{ server ? t('channels.change') : t('channels.mailServer.setUp') }}
        </Button>
      </div>
      <Alert v-if="serverError" variant="destructive">{{ problemMessage(serverError) }}</Alert>
    </div>

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

      <ul
        v-if="channels?.length"
        class="grid grid-cols-[repeat(auto-fill,minmax(18rem,1fr))] gap-3"
        data-testid="channel-grid"
      >
        <li
          v-for="channel in channels"
          :key="channel.id"
          class="flex min-w-0 flex-col gap-1.5 rounded-xl border bg-card p-3"
        >
          <div class="flex min-w-0 items-center gap-2">
            <component
              :is="channel.target.kind === 'email' ? Mail : Webhook"
              class="size-4 shrink-0 text-primary"
            />
            <span class="min-w-0 flex-1 truncate font-medium">{{ channel.name }}</span>
          </div>
          <ul
            v-if="channel.target.kind === 'email'"
            class="grid gap-0.5 text-sm"
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
          <!-- What did not go out: waiting for the next try, and what never will. -->
          <div
            v-if="channel.waiting.length"
            class="grid gap-0.5 text-xs"
            data-testid="channel-waiting"
          >
            <p class="font-medium text-amber-700 dark:text-amber-400">
              {{
                t(
                  'channels.queue.waiting',
                  {
                    n: channel.waiting.length,
                    time: formatDateTime(channel.waiting[0]!.at * 1000),
                  },
                  channel.waiting.length,
                )
              }}
            </p>
            <ul class="grid gap-0.5 text-muted-foreground">
              <li v-for="(item, index) in channel.waiting" :key="index" class="min-w-0 truncate">
                {{ undelivered(item) }}
                <template v-if="item.attempts">
                  · {{ t('channels.queue.tries', { n: item.attempts }, item.attempts) }}
                </template>
              </li>
            </ul>
            <p v-if="channel.waiting[0]!.error" class="text-muted-foreground">
              {{ channel.waiting[0]!.error }}
            </p>
          </div>
          <div
            v-if="channel.failed.length"
            class="grid gap-0.5 text-xs"
            data-testid="channel-failed"
          >
            <p class="font-medium text-destructive">
              {{ t('channels.queue.failed', { n: channel.failed.length }, channel.failed.length) }}
            </p>
            <ul class="grid gap-0.5 text-muted-foreground">
              <li v-for="(item, index) in channel.failed" :key="index" class="min-w-0">
                {{ undelivered(item) }}
                · {{ t('channels.queue.tries', { n: item.attempts }, item.attempts) }}:
                {{ item.error }}
              </li>
            </ul>
          </div>
          <p
            v-if="tests[channel.id] && tests[channel.id] !== 'sending'"
            class="text-xs"
            :class="
              (tests[channel.id] as { ok: boolean }).ok
                ? 'text-emerald-700 dark:text-emerald-400'
                : 'text-destructive'
            "
            role="status"
          >
            {{ (tests[channel.id] as { text: string }).text }}
          </p>
          <div class="mt-auto flex flex-wrap items-center gap-1 pt-1.5">
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
              size="icon-sm"
              variant="ghost"
              class="ml-auto text-muted-foreground hover:text-destructive"
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
