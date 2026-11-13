<script setup lang="ts">
import { LogOut } from 'lucide-vue-next'
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { PageTabs } from '@/shared/ui/page'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useEndSession, useLogins, useSessions } from '../api'
import { describeUserAgent } from '../lib/user-agent'

const open = defineModel<boolean>('open', { default: false })

const i18n = useI18n()
const { t } = i18n
const tab = ref('sessions')
const { data: sessions } = useSessions(() => open.value && tab.value === 'sessions')
const { data: logins } = useLogins(() => open.value && tab.value === 'logins')
const endSession = useEndSession()
watch(open, (isOpen) => {
  if (isOpen) {
    tab.value = 'sessions'
    endSession.reset()
  }
})

const at = (seconds: number) => formatDateTime(seconds * 1000)

/** What came of a login, from its outcome and reason. */
function result(outcome: string, reason: string | null | undefined): string {
  if (outcome === 'success') return t('security.activity.success')
  const key = `security.activity.reasons.${reason ?? ''}`
  return reason && i18n.te(key) ? t(key) : t('security.activity.failed')
}
</script>

<!-- Who is signed in now, and who tried to sign in lately. -->
<template>
  <StepDialog
    v-model:open="open"
    :title="t('security.activity.title')"
    :close-label="t('desktop.close')"
  >
    <template #tabs>
      <div class="px-5">
        <PageTabs
          v-model="tab"
          :label="t('security.activity.title')"
          :tabs="[
            { id: 'sessions', label: t('security.activity.sessions') },
            { id: 'logins', label: t('security.activity.logins') },
          ]"
          class="mb-0"
        />
      </div>
    </template>

    <Alert v-if="endSession.error.value" variant="destructive" class="mb-3">{{
      problemMessage(endSession.error.value)
    }}</Alert>

    <div v-if="tab === 'sessions'" class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('security.activity.account') }}</TableHead>
            <TableHead>{{ t('security.activity.address') }}</TableHead>
            <TableHead>{{ t('security.activity.device') }}</TableHead>
            <TableHead>{{ t('security.activity.lastSeen') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('people.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="session in sessions" :key="session.id">
            <TableCell>
              <span class="flex items-center gap-2">
                <span class="font-medium">{{ session.account }}</span>
                <Badge v-if="session.current" variant="success">{{
                  t('security.activity.current')
                }}</Badge>
              </span>
            </TableCell>
            <TableCell class="font-mono text-dense text-muted-foreground">{{
              session.address ?? '–'
            }}</TableCell>
            <TableCell class="text-muted-foreground">{{
              describeUserAgent(session.user_agent) ?? '–'
            }}</TableCell>
            <TableCell class="text-muted-foreground" :title="at(session.created_at)">{{
              at(session.last_seen_at)
            }}</TableCell>
            <TableCell>
              <Button
                v-if="!session.current"
                variant="ghost"
                size="icon"
                :title="t('security.activity.end')"
                :aria-label="t('security.activity.end')"
                :disabled="endSession.isPending.value"
                @click="endSession.mutate(session.id)"
              >
                <LogOut />
              </Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <div v-else class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('security.activity.time') }}</TableHead>
            <TableHead>{{ t('security.activity.account') }}</TableHead>
            <TableHead>{{ t('security.activity.address') }}</TableHead>
            <TableHead>{{ t('security.activity.result') }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="entry in logins?.entries" :key="entry.id">
            <TableCell class="text-muted-foreground">{{ at(entry.at) }}</TableCell>
            <TableCell class="font-medium">{{ entry.actor ?? '–' }}</TableCell>
            <TableCell class="font-mono text-dense text-muted-foreground">{{
              entry.client_ip ?? '–'
            }}</TableCell>
            <TableCell>
              <Badge :variant="entry.outcome === 'success' ? 'success' : 'destructive'">{{
                result(entry.outcome, entry.reason)
              }}</Badge>
            </TableCell>
          </TableRow>
          <TableRow v-if="logins && !logins.entries.length" class="hover:bg-transparent">
            <TableCell colspan="4" class="py-10 text-center text-muted-foreground">{{
              t('security.activity.noLogins')
            }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <template #footer>
      <Button variant="ghost" @click="open = false">{{ t('desktop.close') }}</Button>
    </template>
  </StepDialog>
</template>
