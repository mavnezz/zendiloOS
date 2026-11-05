<script setup lang="ts">
import { Mail, Pencil, Plus, Send, Trash2, Webhook } from 'lucide-vue-next'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NotificationChannel } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

import { useChannelActions, useChannels } from '../api'
import ChannelDialog from './ChannelDialog.vue'

const { t } = useI18n()
const { data: channels, error } = useChannels()
const actions = useChannelActions()

const editing = ref<NotificationChannel | null>(null)
const dialogOpen = ref(false)
const removing = ref<NotificationChannel | null>(null)
const removeOpen = ref(false)
/** The outcome of the last test, per channel. */
const tests = ref<Record<number, { ok: boolean; text: string } | 'sending'>>({})
const failure = ref<string | null>(null)

function add(): void {
  editing.value = null
  dialogOpen.value = true
}

function edit(channel: NotificationChannel): void {
  editing.value = channel
  dialogOpen.value = true
}

function where(channel: NotificationChannel): string {
  const target = channel.target
  if (target.kind === 'email') return `${target.to.join(', ')} · ${target.server}`
  try {
    return new URL(target.url).host
  } catch {
    return target.url
  }
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

<!-- Where notifications go besides the interface: email and webhooks. -->
<template>
  <div class="grid gap-4">
    <div class="flex items-start gap-3">
      <p class="flex-1 text-sm text-muted-foreground">{{ t('channels.hint') }}</p>
      <Button size="sm" @click="add">
        <Plus />
        {{ t('channels.add') }}
      </Button>
    </div>
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
          <Badge variant="secondary">{{ t(`channels.levels.${channel.min_level}`) }}</Badge>
        </div>
        <p class="truncate text-sm text-muted-foreground">{{ where(channel) }}</p>
        <p v-if="channel.last_delivery_at" class="text-xs text-muted-foreground">
          <template v-if="channel.last_error">
            <span class="text-destructive">
              {{
                t('channels.lastFailed', { time: formatDateTime(channel.last_delivery_at * 1000) })
              }}
            </span>
            {{ channel.last_error }}
          </template>
          <template v-else>
            {{ t('channels.lastSent', { time: formatDateTime(channel.last_delivery_at * 1000) }) }}
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
