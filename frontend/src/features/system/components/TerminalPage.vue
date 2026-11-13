<script setup lang="ts">
import { Copy, Settings2 } from 'lucide-vue-next'
import { computed, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SshState } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'
import { PageDescription, PageFooter, PageHeader, SettingRow } from '@/shared/ui/page'
import { Switch } from '@/shared/ui/switch'

import { useSetSsh, useSsh } from '../api'
import { fromDraft, OFF_AFTER, portValid, sameSsh, type SshDraft, toDraft } from '../lib/ssh'
import SshOptionsDialog from './SshOptionsDialog.vue'

/** Where clients reach the NAS: its name, and its address if known. */
const props = defineProps<{ hostname?: string; address?: string }>()

const { t, locale } = useI18n()
const notifications = useNotifications()
const { data, isPending } = useSsh()
const setSsh = useSetSsh()

const advanced = ref(false)
const draft = ref<SshDraft>()
/** The settings the draft started from. */
const base = shallowRef<SshDraft>()

function reset(state: SshState | undefined = data.value): void {
  if (!state) return
  base.value = toDraft(state)
  draft.value = toDraft(state)
  setSsh.reset()
}
const dirty = computed(() =>
  Boolean(draft.value && base.value && !sameSsh(draft.value, base.value)),
)
watch(
  data,
  (state) => {
    if (!base.value || !dirty.value) reset(state)
  },
  { immediate: true },
)

const installed = computed(() => data.value?.installed ?? true)
const portOk = computed(() => portValid(draft.value?.port ?? ''))

function apply(ssh: SshDraft | undefined = draft.value): void {
  if (!ssh) return
  const request = fromDraft(ssh, data.value?.off_at, Math.floor(Date.now() / 1000))
  setSsh.mutate(request, {
    onSuccess: (state) => {
      reset(state)
      advanced.value = false
    },
  })
}

function clockTime(seconds: number): string {
  return new Intl.DateTimeFormat(locale.value, { hour: '2-digit', minute: '2-digit' }).format(
    seconds * 1000,
  )
}
const offAt = computed(() => (data.value?.off_at != null ? clockTime(data.value.off_at) : null))

const status = computed(() => {
  const state = data.value
  if (!state?.installed) return { variant: 'outline' as const, text: t('terminal.notInstalled') }
  if (state.running) return { variant: 'success' as const, text: t('terminal.running') }
  if (state.enabled) return { variant: 'destructive' as const, text: t('terminal.failed') }
  return { variant: 'secondary' as const, text: t('terminal.stopped') }
})

/** The command to connect with, for the port as it is now. */
const commands = computed(() => {
  const port = data.value?.port ?? 22
  const option = port === 22 ? '' : `-p ${port} `
  const user = t('terminal.user')
  const hosts = [`${props.hostname ?? 'nas'}.local`, ...(props.address ? [props.address] : [])]
  return hosts.map((host) => `ssh ${option}${user}@${host}`)
})
async function copy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
    notifications.notify('success', t('fileServices.copied', { text }))
  } catch {
    notifications.notify('error', t('fileServices.copyFailed'))
  }
}
</script>

<!--
  SSH as a service of the NAS: on or off, its port, a timer to turn it off
  again, and who may log in how behind "Advanced".
-->
<template>
  <div class="flex min-h-full flex-col">
    <PageHeader :title="t('terminal.title')" :description="t('terminal.description')" />

    <div
      v-if="isPending || !draft || !data"
      class="h-64 max-w-2xl animate-pulse rounded-lg bg-muted"
    />

    <template v-else>
      <PageDescription>
        <p>{{ t('terminal.explanation') }}</p>
        <p>{{ t('terminal.sessionsStay') }}</p>
      </PageDescription>
      <Alert v-if="!installed" class="mb-5 max-w-3xl">
        {{ t('terminal.installHint') }}
        <code class="font-mono text-dense">apt install openssh-server</code>
      </Alert>

      <div class="grid max-w-3xl gap-5">
        <SettingRow label="SSH" for="ssh-enabled">
          <div class="flex flex-wrap items-center gap-3 pt-1 text-sm">
            <label class="flex items-center gap-3">
              <Switch id="ssh-enabled" v-model="draft.enabled" :disabled="!installed" />
              {{ t('terminal.enable') }}
            </label>
            <Badge :variant="status.variant">{{ status.text }}</Badge>
          </div>
          <p class="text-xs text-muted-foreground">{{ t('terminal.enableHint') }}</p>
        </SettingRow>

        <SettingRow :label="t('terminal.port')" for="ssh-port">
          <Input
            id="ssh-port"
            v-model="draft.port"
            inputmode="numeric"
            maxlength="5"
            autocomplete="off"
            class="w-28 font-mono text-dense"
            :disabled="!installed || !draft.enabled"
            :aria-invalid="!portOk"
          />
          <p class="text-xs" :class="portOk ? 'text-muted-foreground' : 'text-destructive'">
            {{ portOk ? t('terminal.portHint') : t('terminal.portInvalid') }}
          </p>
        </SettingRow>

        <SettingRow :label="t('terminal.offAfter')" for="ssh-off">
          <NativeSelect
            id="ssh-off"
            v-model="draft.off_after"
            class="w-64"
            :disabled="!installed || !draft.enabled"
          >
            <option value="never">{{ t('terminal.never') }}</option>
            <option v-if="offAt && base?.off_after === 'planned'" value="planned">
              {{ t('terminal.planned', { time: offAt }) }}
            </option>
            <option v-for="minutes in OFF_AFTER" :key="minutes" :value="String(minutes)">
              {{
                minutes < 60
                  ? t('terminal.afterMinutes', { n: minutes })
                  : t('terminal.afterHours', { n: minutes / 60 }, minutes / 60)
              }}
            </option>
          </NativeSelect>
          <p class="text-xs text-muted-foreground">
            {{
              offAt && data.running
                ? t('terminal.offAt', { time: offAt })
                : t('terminal.offAfterHint')
            }}
          </p>
        </SettingRow>

        <SettingRow label="">
          <Button variant="secondary" class="w-fit" :disabled="!installed" @click="advanced = true">
            <Settings2 />
            {{ t('terminal.advanced.title') }}
          </Button>
        </SettingRow>
      </div>

      <section class="mt-8 max-w-3xl border-t pt-5">
        <h3 class="mb-2 text-sm font-medium">{{ t('terminal.connect') }}</h3>
        <div class="flex flex-wrap items-center gap-2">
          <template v-for="(command, index) in commands" :key="command">
            <span v-if="index > 0" class="text-sm text-muted-foreground">{{
              t('fileServices.or')
            }}</span>
            <button
              type="button"
              class="group inline-flex items-center gap-1.5 rounded-md bg-muted px-2 py-0.5 font-mono text-dense hover:bg-accent"
              :title="t('fileServices.copy')"
              @click="copy(command)"
            >
              {{ command }}
              <Copy class="size-3.5 text-muted-foreground group-hover:text-foreground" />
            </button>
          </template>
        </div>
      </section>
    </template>

    <PageFooter
      v-if="draft"
      :dirty="dirty"
      :saving="setSsh.isPending.value"
      :valid="portOk"
      :error="setSsh.error.value && !advanced ? problemMessage(setSsh.error.value) : null"
      @reset="reset()"
      @apply="apply()"
    />
    <SshOptionsDialog
      v-if="draft"
      v-model:open="advanced"
      :ssh="draft"
      :saving="setSsh.isPending.value"
      :error="setSsh.error.value ? problemMessage(setSsh.error.value) : null"
      @save="apply"
    />
  </div>
</template>
