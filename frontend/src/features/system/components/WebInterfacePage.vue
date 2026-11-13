<script setup lang="ts">
import { computed, onUnmounted, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Certificate, WebServer } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'
import { PageDescription, PageFooter, PageHeader, SettingRow } from '@/shared/ui/page'
import { Switch } from '@/shared/ui/switch'

import { useWebCertificates, useWebServer, useWebServerActions } from '../api'

/** Whether the signed-in account may change the settings (`system.manage`). */
const props = defineProps<{ canManage: boolean }>()

const { t } = useI18n()
const { data: setup, error: loadError } = useWebServer()
const { data: certificates } = useWebCertificates(() => props.canManage)
const actions = useWebServerActions()

interface Draft {
  http: boolean
  httpPort: string
  httpsPort: string
  /** 0: the machine's own certificate. */
  certificate: number
}
const draft = ref<Draft>()
const base = shallowRef<Draft>()
const saving = ref(false)
const failure = ref<string | null>(null)
/** Where the interface answers after the change just applied, if not here. */
const newAddress = ref<string | null>(null)

function toDraft(web: WebServer): Draft {
  return {
    http: web.http_port !== null && web.http_port !== undefined,
    httpPort: String(web.http_port ?? 80),
    httpsPort: String(web.https_port),
    certificate: web.certificate_id ?? 0,
  }
}
const same = (a: Draft, b: Draft) =>
  a.http === b.http &&
  (!a.http || a.httpPort.trim() === b.httpPort.trim()) &&
  a.httpsPort.trim() === b.httpsPort.trim() &&
  a.certificate === b.certificate
const dirty = computed(() => Boolean(draft.value && base.value && !same(draft.value, base.value)))

function reset(): void {
  if (!setup.value) return
  base.value = toDraft(setup.value.current)
  draft.value = toDraft(setup.value.current)
  failure.value = null
}
watch(
  setup,
  () => {
    if (!base.value || !dirty.value) reset()
  },
  { immediate: true },
)

const port = (text: string) => (/^\d{1,5}$/.test(text.trim()) ? Number(text.trim()) : NaN)
const portOk = (text: string) => port(text) >= 1 && port(text) <= 65535
const problem = computed(() => {
  const value = draft.value
  if (!value) return null
  if (!portOk(value.httpsPort) || (value.http && !portOk(value.httpPort)))
    return t('webServer.invalidPort')
  if (value.http && port(value.httpPort) === port(value.httpsPort)) return t('webServer.samePorts')
  return null
})

/** Certificates that hold, and the one shown now even if it ended. */
const offered = computed<Certificate[]>(() =>
  (certificates.value ?? []).filter(
    (c) => (c.not_after ?? 0) * 1000 > Date.now() || c.id === setup.value?.current.certificate_id,
  ),
)
const label = (c: Certificate) =>
  t('webServer.issued', {
    name: c.domains.join(', '),
    date: formatDateTime((c.not_after ?? 0) * 1000),
  })

/** The port this page came from. */
function herePort(): number {
  const { port: here, protocol } = window.location
  return here ? Number(here) : protocol === 'https:' ? 443 : 80
}

async function apply(): Promise<void> {
  const value = draft.value
  if (!value || problem.value) return
  saving.value = true
  failure.value = null
  const https = port(value.httpsPort)
  try {
    await actions.apply({
      http_port: value.http ? port(value.httpPort) : null,
      https_port: https,
      certificate_id: value.certificate || null,
    })
    // Applied: nothing left to apply. Gone back unconfirmed, the page shows the old settings again.
    base.value = { ...value }
    newAddress.value =
      https === herePort()
        ? null
        : `https://${window.location.hostname}${https === 443 ? '' : `:${https}`}/`
  } catch (error) {
    failure.value = problemMessage(error)
  } finally {
    saving.value = false
  }
}

async function confirm(): Promise<void> {
  failure.value = null
  try {
    await actions.confirm()
    newAddress.value = null
    base.value = undefined
  } catch (error) {
    failure.value = problemMessage(error)
  }
}

// A clock for the countdown.
const now = ref(Date.now())
const ticking = setInterval(() => (now.value = Date.now()), 1000)
onUnmounted(() => clearInterval(ticking))
const secondsLeft = computed(() => {
  const at = setup.value?.pending?.revert_at
  return at ? Math.max(0, Math.round(at - now.value / 1000)) : 0
})

const number = 'w-24 font-mono text-dense'
</script>

<!--
  Where the web interface answers: plain HTTP (only ever pointing to HTTPS)
  or not, the HTTPS port, and the certificate nginx shows. A change applies
  at once and goes back by itself unless confirmed from where it answers.
-->
<template>
  <div class="flex min-h-full flex-col">
    <PageHeader :title="t('controlPanel.webInterface')" :description="t('webServer.description')" />
    <PageDescription>
      <p>{{ t('webServer.help') }}</p>
    </PageDescription>
    <div class="grid max-w-3xl gap-6">
      <Alert v-if="loadError" variant="destructive">{{ problemMessage(loadError) }}</Alert>
      <div
        v-if="setup?.pending"
        class="grid gap-2 rounded-xl border border-amber-500/50 bg-amber-500/10 p-4 text-sm"
        role="status"
      >
        <p class="font-medium">{{ t('webServer.pending.title') }}</p>
        <p>{{ t('webServer.pending.hint', { seconds: secondsLeft }) }}</p>
        <p v-if="newAddress">
          {{ t('webServer.pending.elsewhere') }}
          <a class="font-medium underline" :href="newAddress">{{ newAddress }}</a>
        </p>
        <div v-if="canManage" class="flex justify-end">
          <Button size="sm" @click="confirm">{{ t('webServer.pending.confirm') }}</Button>
        </div>
      </div>

      <div v-if="!draft" class="h-48 animate-pulse rounded-lg bg-muted" />
      <template v-else>
        <SettingRow :label="t('webServer.http')" for="web-http">
          <label class="flex items-center gap-3 pt-1 text-sm">
            <Switch id="web-http" v-model="draft.http" :disabled="!canManage || !!setup?.pending" />
            {{ draft.http ? t('webServer.httpRedirect') : t('webServer.httpOff') }}
          </label>
          <div class="flex items-center gap-2 text-sm">
            <span class="text-muted-foreground">{{ t('webServer.httpPort') }}</span>
            <Input
              v-model="draft.httpPort"
              inputmode="numeric"
              :class="number"
              :disabled="!canManage || !draft.http || !!setup?.pending"
              :aria-label="t('webServer.httpPort')"
              :aria-invalid="draft.http && !portOk(draft.httpPort)"
            />
          </div>
          <p class="text-xs text-muted-foreground">{{ t('webServer.httpHint') }}</p>
        </SettingRow>

        <SettingRow :label="t('webServer.httpsPort')" for="web-https">
          <Input
            id="web-https"
            v-model="draft.httpsPort"
            inputmode="numeric"
            :class="number"
            :disabled="!canManage || !!setup?.pending"
            :aria-invalid="!portOk(draft.httpsPort)"
          />
          <p class="text-xs text-muted-foreground">{{ t('webServer.portHint') }}</p>
        </SettingRow>

        <SettingRow :label="t('webServer.certificate')" for="web-certificate">
          <NativeSelect
            id="web-certificate"
            v-model="draft.certificate"
            class="max-w-md"
            :disabled="!canManage || !!setup?.pending"
          >
            <option :value="0">{{ t('webServer.own') }}</option>
            <option v-for="c in offered" :key="c.id" :value="c.id">{{ label(c) }}</option>
          </NativeSelect>
          <p class="text-xs text-muted-foreground">
            {{ offered.length ? t('webServer.certificateHint') : t('webServer.noCertificates') }}
          </p>
        </SettingRow>
      </template>
    </div>
    <div class="grow" aria-hidden="true" />
    <PageFooter
      v-if="draft && canManage"
      :dirty="dirty"
      :saving="saving"
      :valid="problem === null"
      :error="failure ?? problem"
      @reset="reset()"
      @apply="apply()"
    />
  </div>
</template>
