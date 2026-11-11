<script setup lang="ts">
import { Copy, KeyRound, LoaderCircle, ShieldCheck } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BackupHostKey } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { checkServer, hostKeys, publicKey } from '../api'
import {
  DAEMON_PORT,
  SSH_PORT,
  type ServerForm,
  type Transport,
  isHost,
  isWord,
} from '../lib/backup'

/**
 * A server: how it is reached, by whom; for SSH the NAS's key to put on it
 * and the server's host key compared and pinned. `path`: the folder on it,
 * for checking; `task`: the task whose stored password a check may use.
 */
const props = defineProps<{ path: string; task?: number; passwordSet?: boolean }>()
const server = defineModel<ServerForm>({ required: true })

const { t } = useI18n()
const key = ref<string | null>(null)
const offered = ref<BackupHostKey[] | null>(null)
const busy = ref<'key' | 'scan' | 'check' | null>(null)
const failure = ref<string | null>(null)
const checked = ref<string | null>(null)
const copied = ref(false)

const field = (name: keyof ServerForm, value: unknown) =>
  (server.value = { ...server.value, [name]: value })

function transport(next: Transport): void {
  const port =
    server.value.port === (next === 'ssh' ? DAEMON_PORT : SSH_PORT)
      ? next === 'ssh'
        ? SSH_PORT
        : DAEMON_PORT
      : server.value.port
  server.value = { ...server.value, transport: next, port, hostKey: null, fingerprint: null }
  offered.value = null
}

async function busyWith(what: 'key' | 'scan' | 'check', work: () => Promise<void>): Promise<void> {
  busy.value = what
  failure.value = null
  try {
    await work()
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = null
  }
}

const showKey = () => busyWith('key', async () => void (key.value = (await publicKey()).public_key))
const scan = () =>
  busyWith(
    'scan',
    async () => void (offered.value = await hostKeys(server.value.host.trim(), server.value.port)),
  )

function pin(chosen: BackupHostKey): void {
  server.value = { ...server.value, hostKey: chosen.line, fingerprint: chosen.fingerprint }
  offered.value = null
}

const reachable = computed(
  () =>
    isHost(server.value.host) &&
    isWord(server.value.user) &&
    (server.value.transport === 'ssh'
      ? Boolean(server.value.hostKey)
      : isWord(server.value.module)),
)

const check = () =>
  busyWith('check', async () => {
    const s = server.value
    const body =
      s.transport === 'daemon'
        ? {
            transport: 'daemon' as const,
            host: s.host.trim(),
            port: s.port,
            user: s.user.trim(),
            module: s.module.trim(),
          }
        : {
            transport: 'ssh' as const,
            host: s.host.trim(),
            port: s.port,
            user: s.user.trim(),
            host_key: s.hostKey,
            fingerprint: s.fingerprint,
          }
    const entries = await checkServer(
      body,
      props.path.trim(),
      s.password || null,
      props.task ?? null,
    )
    checked.value = entries.length
      ? t('backup.server.checkOk', entries.length)
      : t('backup.server.checkEmpty')
  })

async function copy(): Promise<void> {
  if (!key.value) return
  await navigator.clipboard?.writeText(key.value)
  copied.value = true
}
</script>

<template>
  <fieldset class="grid gap-4 rounded-xl border p-4">
    <legend class="px-1 text-sm font-medium">{{ t('backup.server.title') }}</legend>
    <div class="grid gap-2" role="radiogroup" :aria-label="t('backup.server.transport')">
      <label
        v-for="choice in ['ssh', 'daemon'] as const"
        :key="choice"
        class="flex items-center gap-2 text-sm"
      >
        <input
          type="radio"
          name="backup-transport"
          :value="choice"
          :checked="server.transport === choice"
          @change="transport(choice)"
        />
        {{ t(`backup.server.transports.${choice}`) }}
      </label>
    </div>
    <Alert v-if="server.transport === 'daemon'">{{ t('backup.server.daemonWarning') }}</Alert>
    <div class="grid gap-3 @lg:grid-cols-[1fr_7rem]">
      <div class="grid gap-1.5">
        <Label for="backup-host">{{ t('backup.server.host') }}</Label>
        <Input
          id="backup-host"
          :model-value="server.host"
          :placeholder="t('backup.server.hostPlaceholder')"
          @update:model-value="(value) => field('host', String(value))"
        />
      </div>
      <div class="grid gap-1.5">
        <Label for="backup-port">{{ t('backup.server.port') }}</Label>
        <Input
          id="backup-port"
          type="number"
          :model-value="server.port"
          @update:model-value="(value) => field('port', Number(value))"
        />
      </div>
    </div>
    <div class="grid gap-3 @lg:grid-cols-2">
      <div class="grid gap-1.5">
        <Label for="backup-user">{{ t('backup.server.user') }}</Label>
        <Input
          id="backup-user"
          :model-value="server.user"
          @update:model-value="(value) => field('user', String(value))"
        />
      </div>
      <div v-if="server.transport === 'daemon'" class="grid gap-1.5">
        <Label for="backup-module">{{ t('backup.server.module') }}</Label>
        <Input
          id="backup-module"
          :model-value="server.module"
          @update:model-value="(value) => field('module', String(value))"
        />
      </div>
    </div>
    <div v-if="server.transport === 'daemon'" class="grid gap-1.5">
      <Label for="backup-password">{{ t('backup.server.password') }}</Label>
      <Input
        id="backup-password"
        type="password"
        autocomplete="new-password"
        :model-value="server.password"
        :placeholder="passwordSet ? t('backup.server.passwordKept') : ''"
        @update:model-value="(value) => field('password', String(value))"
      />
    </div>

    <template v-if="server.transport === 'ssh'">
      <div class="grid gap-2 rounded-lg bg-muted/50 p-3 text-sm">
        <span class="flex items-center gap-2 font-medium"
          ><KeyRound class="size-4" /> {{ t('backup.server.publicKey') }}</span
        >
        <p class="text-xs text-muted-foreground">{{ t('backup.server.publicKeyHint') }}</p>
        <div v-if="key" class="flex items-start gap-2">
          <code
            class="min-w-0 flex-1 rounded bg-background p-2 text-xs break-all"
            data-testid="backup-public-key"
            >{{ key }}</code
          >
          <Button type="button" size="sm" variant="outline" @click="copy"
            ><Copy /> {{ copied ? t('backup.server.copied') : t('backup.server.copy') }}</Button
          >
        </div>
        <Button
          v-else
          type="button"
          size="sm"
          variant="outline"
          class="w-fit"
          :disabled="busy === 'key'"
          @click="showKey"
          >{{ t('backup.server.makeKey') }}</Button
        >
      </div>
      <div class="grid gap-2 text-sm">
        <p class="text-xs text-muted-foreground">{{ t('backup.server.hostKeyHint') }}</p>
        <p
          v-if="server.hostKey && !offered"
          class="flex flex-wrap items-center gap-2"
          data-testid="backup-pinned"
        >
          <ShieldCheck class="size-4 text-emerald-600" />
          <span class="font-mono text-xs break-all">{{
            t('backup.server.pinned', { fingerprint: server.fingerprint ?? '' })
          }}</span>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            :disabled="!isHost(server.host)"
            @click="scan"
            >{{ t('backup.server.changeKey') }}</Button
          >
        </p>
        <Button
          v-else-if="!offered"
          type="button"
          size="sm"
          variant="outline"
          class="w-fit"
          :disabled="!isHost(server.host) || busy === 'scan'"
          @click="scan"
        >
          <LoaderCircle v-if="busy === 'scan'" class="animate-spin" />
          {{ t('backup.server.fetchHostKey') }}
        </Button>
        <ul v-if="offered" class="grid gap-2">
          <li
            v-for="offer in offered"
            :key="offer.line"
            class="flex flex-wrap items-center gap-2 rounded-lg border p-2"
          >
            <span class="grid min-w-0 flex-1 gap-0.5">
              <span class="text-xs text-muted-foreground">{{ offer.line.split(' ')[0] }}</span>
              <span class="font-mono text-xs break-all">{{ offer.fingerprint }}</span>
            </span>
            <Button type="button" size="sm" @click="pin(offer)">{{
              t('backup.server.confirmKey')
            }}</Button>
          </li>
        </ul>
      </div>
    </template>
    <div class="flex flex-wrap items-center gap-2">
      <Button
        type="button"
        size="sm"
        variant="outline"
        :disabled="!reachable || busy === 'check'"
        @click="check"
      >
        <LoaderCircle v-if="busy === 'check'" class="animate-spin" />
        {{ t('backup.server.check') }}
      </Button>
      <span v-if="checked" class="text-sm text-emerald-600 dark:text-emerald-400">{{
        checked
      }}</span>
    </div>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
  </fieldset>
</template>
