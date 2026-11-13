<script setup lang="ts">
import { Plus, ShieldCheck, Trash2 } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SecuritySettings } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { PageTabs } from '@/shared/ui/page'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useBlock, useBlocked, useSetSecuritySettings, useUnblock } from '../api'

/** An address or network, roughly; the server checks it exactly. */
const RANGE = /^[0-9a-fA-F:.]+(\/\d{1,3})?$/

const props = defineProps<{ settings: SecuritySettings }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const notifications = useNotifications()
const { data: blocked } = useBlocked(() => open.value)
const block = useBlock()
const unblock = useUnblock()
const setSettings = useSetSecuritySettings()

const tab = ref('blocked')
const address = ref('')
/** The allowed addresses as edited here; saved with "Save". */
const allowed = ref<string[]>([])
const allowing = ref('')
watch(open, (isOpen) => {
  if (!isOpen) return
  tab.value = 'blocked'
  address.value = ''
  allowing.value = ''
  allowed.value = [...(props.settings.allowed ?? [])]
  block.reset()
  unblock.reset()
  setSettings.reset()
})

const allowedChanged = computed(
  () => JSON.stringify(allowed.value) !== JSON.stringify(props.settings.allowed ?? []),
)
const error = computed(() => block.error.value ?? unblock.error.value ?? setSettings.error.value)

function add(): void {
  const value = address.value.trim()
  if (!RANGE.test(value)) return
  block.mutate(value, {
    onSuccess: (entry) => {
      address.value = ''
      notifications.notify('success', t('security.blocks.blocked', { address: entry.address }))
    },
  })
}

function lift(entry: string): void {
  unblock.mutate(entry, {
    onSuccess: () =>
      notifications.notify('success', t('security.blocks.lifted', { address: entry })),
  })
}

function allow(): void {
  const value = allowing.value.trim()
  if (!RANGE.test(value) || allowed.value.includes(value)) return
  allowed.value = [...allowed.value, value]
  allowing.value = ''
}

function save(): void {
  setSettings.mutate(
    { ...props.settings, allowed: allowed.value },
    { onSuccess: (stored) => (allowed.value = [...(stored.allowed ?? [])]) },
  )
}

const until = (seconds: number | null | undefined) =>
  seconds == null ? t('security.blocks.untilLifted') : formatDateTime(seconds * 1000)
</script>

<!--
  The addresses the web interface refuses, blocked after failed logins or
  by hand, and those it never blocks.
-->
<template>
  <StepDialog
    v-model:open="open"
    :title="t('security.blocks.title')"
    :close-label="t('desktop.close')"
  >
    <template #tabs>
      <div class="px-5">
        <PageTabs
          v-model="tab"
          :label="t('security.blocks.title')"
          :tabs="[
            { id: 'blocked', label: t('security.blocks.blockedTab') },
            { id: 'allowed', label: t('security.blocks.allowedTab') },
          ]"
          class="mb-0"
        />
      </div>
    </template>

    <Alert v-if="error" variant="destructive" class="mb-3">{{ problemMessage(error) }}</Alert>

    <div v-if="tab === 'blocked'" class="grid gap-3 py-1">
      <form class="flex flex-wrap items-center gap-2" @submit.prevent="add">
        <Input
          v-model="address"
          class="w-80 font-mono text-dense"
          :placeholder="t('security.blocks.addressPlaceholder')"
          :aria-label="t('security.blocks.address')"
          spellcheck="false"
        />
        <Button type="submit" :disabled="!RANGE.test(address.trim()) || block.isPending.value">
          <Plus />
          {{ t('security.blocks.block') }}
        </Button>
      </form>
      <div class="overflow-hidden rounded-xl border bg-card">
        <Table>
          <TableHeader class="bg-muted/50">
            <TableRow class="hover:bg-transparent">
              <TableHead>{{ t('security.blocks.address') }}</TableHead>
              <TableHead>{{ t('security.blocks.since') }}</TableHead>
              <TableHead>{{ t('security.blocks.until') }}</TableHead>
              <TableHead class="w-12"
                ><span class="sr-only">{{ t('people.actions') }}</span></TableHead
              >
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="entry in blocked" :key="entry.address">
              <TableCell>
                <span class="flex items-center gap-2">
                  <span class="font-mono text-dense">{{ entry.address }}</span>
                  <Badge :variant="entry.automatic ? 'warning' : 'secondary'">{{
                    entry.automatic ? t('security.blocks.automatic') : t('security.blocks.manual')
                  }}</Badge>
                </span>
              </TableCell>
              <TableCell class="text-muted-foreground">{{
                formatDateTime(entry.blocked_at * 1000)
              }}</TableCell>
              <TableCell class="text-muted-foreground">{{ until(entry.until) }}</TableCell>
              <TableCell>
                <Button
                  variant="ghost"
                  size="icon"
                  :title="t('security.blocks.lift')"
                  :aria-label="t('security.blocks.lift')"
                  :disabled="unblock.isPending.value"
                  @click="lift(entry.address)"
                >
                  <Trash2 />
                </Button>
              </TableCell>
            </TableRow>
            <TableRow v-if="!blocked?.length" class="hover:bg-transparent">
              <TableCell colspan="4" class="py-10 text-center text-muted-foreground">
                <ShieldCheck class="mx-auto mb-2 size-6 opacity-60" />
                {{ t('security.blocks.none') }}
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </div>

    <div v-else class="grid gap-3 py-1">
      <p class="text-sm text-muted-foreground">{{ t('security.blocks.allowedHint') }}</p>
      <form class="flex flex-wrap items-center gap-2" @submit.prevent="allow">
        <Input
          v-model="allowing"
          class="w-64 font-mono text-dense"
          :placeholder="t('security.blocks.networkPlaceholder')"
          :aria-label="t('security.blocks.address')"
          spellcheck="false"
        />
        <Button type="submit" variant="secondary" :disabled="!RANGE.test(allowing.trim())">
          <Plus />
          {{ t('security.blocks.allow') }}
        </Button>
      </form>
      <ul class="divide-y rounded-xl border bg-card">
        <li v-for="entry in allowed" :key="entry" class="flex items-center gap-2 px-4 py-2">
          <span class="flex-1 font-mono text-dense">{{ entry }}</span>
          <Button
            variant="ghost"
            size="icon"
            :title="t('security.blocks.remove')"
            :aria-label="t('security.blocks.remove')"
            @click="allowed = allowed.filter((item) => item !== entry)"
          >
            <Trash2 />
          </Button>
        </li>
        <li v-if="!allowed.length" class="px-4 py-6 text-center text-sm text-muted-foreground">
          {{ t('security.blocks.noneAllowed') }}
        </li>
      </ul>
    </div>

    <template #footer>
      <Button variant="ghost" @click="open = false">{{ t('desktop.close') }}</Button>
      <Button
        v-if="tab === 'allowed'"
        :disabled="!allowedChanged || setSettings.isPending.value"
        @click="save"
        >{{ t('security.blocks.save') }}</Button
      >
    </template>
  </StepDialog>
</template>
