<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { LoaderCircle, Search } from 'lucide-vue-next'
import { computed, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type {
  UpsDevice,
  UpsOverview,
  UpsRequest,
  UpsShutdown,
  UpsSourceInput,
} from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { InfoList, type InfoItem } from '@/shared/ui/info-list'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'
import { PageCard, PageDescription, PageFooter, PageHeader, SettingRow } from '@/shared/ui/page'
import { RadioGroup, RadioGroupItem } from '@/shared/ui/radio-group'
import { Switch } from '@/shared/ui/switch'

import { installUps, scanUps, useSetUps, useUps } from '../api'
import UpsStatus from './UpsStatus.vue'

/**
 * Whether the signed-in account may change the settings (`system.manage`),
 * and where other devices reach this NAS.
 */
const props = defineProps<{ canManage: boolean; address?: string }>()

const { t } = useI18n()
const client = useQueryClient()
const jobs = useJobsStore()
const { data: overview, error: loadError } = useUps()
const setUps = useSetUps()

type Kind = 'none' | UpsSourceInput['kind']
/** Where the UPS's driver runs on this NAS: only those can be told things. */
const LOCAL: Kind[] = ['usb', 'snmp', 'simulated']

interface Draft {
  kind: Kind
  driver: string
  /** Empty: any UPS the driver finds. */
  vendorId: string
  productId: string
  serial: string
  address: string
  community: string
  version: 'v1' | 'v2c'
  host: string
  port: string
  ups: string
  user: string
  /** Empty: the one kept stays. */
  password: string
  when: UpsShutdown['kind']
  percent: string
  minutes: string
  cutPower: boolean
  serve: boolean
  serveUser: string
  servePassword: string
}

function toDraft(from: UpsOverview): Draft {
  const { source, shutdown, cut_power: cutPower, serve } = from.settings
  const draft: Draft = {
    kind: source?.kind ?? 'none',
    driver: from.drivers[0] ?? '',
    vendorId: '',
    productId: '',
    serial: '',
    address: '',
    community: 'public',
    version: 'v2c',
    host: '',
    port: String(from.port),
    ups: from.name,
    user: from.default_account,
    password: '',
    when: shutdown.kind,
    percent: shutdown.kind === 'charge_below' ? String(shutdown.percent) : '30',
    minutes: shutdown.kind === 'after_minutes' ? String(shutdown.minutes) : '10',
    cutPower,
    serve: Boolean(serve),
    serveUser: serve?.user ?? from.default_account,
    servePassword: '',
  }
  if (source?.kind === 'usb') {
    draft.driver = source.driver
    draft.vendorId = source.vendor_id ?? ''
    draft.productId = source.product_id ?? ''
    draft.serial = source.serial ?? ''
  } else if (source?.kind === 'snmp') {
    draft.address = source.address
    draft.community = source.community
    draft.version = source.version
  } else if (source?.kind === 'server') {
    draft.host = source.host
    draft.port = String(source.port)
    draft.ups = source.ups
    draft.user = source.user
  }
  return draft
}

function toRequest(value: Draft): UpsRequest {
  const local = LOCAL.includes(value.kind)
  let source: UpsSourceInput | null = null
  if (value.kind === 'usb')
    source = {
      kind: 'usb',
      driver: value.driver,
      vendor_id: value.vendorId || null,
      product_id: value.productId || null,
      serial: value.serial || null,
    }
  else if (value.kind === 'snmp')
    source = {
      kind: 'snmp',
      address: value.address.trim(),
      community: value.community.trim(),
      version: value.version,
    }
  else if (value.kind === 'server')
    source = {
      kind: 'server',
      host: value.host.trim(),
      port: Number(value.port),
      ups: value.ups.trim(),
      user: value.user.trim(),
      password: value.password || null,
    }
  else if (value.kind === 'simulated') source = { kind: 'simulated' }
  const shutdown: UpsShutdown =
    value.when === 'charge_below'
      ? { kind: 'charge_below', percent: Number(value.percent) }
      : value.when === 'after_minutes'
        ? { kind: 'after_minutes', minutes: Number(value.minutes) }
        : { kind: 'low_battery' }
  return {
    source,
    shutdown,
    cut_power: local && value.cutPower,
    serve:
      local && value.serve
        ? { user: value.serveUser.trim(), password: value.servePassword || null }
        : null,
  }
}

const draft = ref<Draft>()
const base = shallowRef<Draft>()
const failure = ref<string | null>(null)
const same = (a: Draft, b: Draft) => JSON.stringify(a) === JSON.stringify(b)
const dirty = computed(() => Boolean(draft.value && base.value && !same(draft.value, base.value)))

function adopt(from: UpsOverview): void {
  base.value = toDraft(from)
  draft.value = toDraft(from)
  failure.value = null
}
function reset(): void {
  if (overview.value) adopt(overview.value)
}
// The state is looked at again and again; the form follows only what was
// changed elsewhere, and never while something waits to be applied here.
watch(
  overview,
  (now) => {
    if (now && (!base.value || (!dirty.value && !same(toDraft(now), base.value)))) adopt(now)
  },
  { immediate: true },
)

const local = computed(() => LOCAL.includes(draft.value?.kind ?? 'none'))
// A charge to shut down at is the driver's to watch: not for a UPS elsewhere.
watch(
  () => draft.value?.kind,
  (kind) => {
    if (draft.value && kind === 'server' && draft.value.when === 'charge_below')
      draft.value.when = 'low_battery'
  },
)

/** The passwords kept, each for the account (and host) it belongs to. */
const kept = computed(() => {
  const settings = overview.value?.settings
  const source = settings?.source
  const value = draft.value
  return {
    server:
      source?.kind === 'server' &&
      source.password_set &&
      source.host === value?.host.trim().toLowerCase() &&
      source.user === value.user.trim(),
    serve: Boolean(
      settings?.serve?.password_set && settings.serve.user === value?.serveUser.trim(),
    ),
  }
})

const whole = (text: string, min: number, max: number) =>
  /^\d{1,5}$/.test(text.trim()) && Number(text) >= min && Number(text) <= max
const problem = computed(() => {
  const value = draft.value
  if (!value || value.kind === 'none') return null
  if (value.kind === 'snmp') {
    if (!overview.value?.snmp) return t('errors.ups_snmp_missing')
    if (!(value.address.trim() && value.community.trim())) return t('ups.problems.address')
  }
  if (value.kind === 'server') {
    if (!(value.host.trim() && value.ups.trim() && value.user.trim()))
      return t('ups.problems.address')
    if (!whole(value.port, 1, 65535)) return t('ups.problems.port')
    if (!value.password && !kept.value.server) return t('ups.problems.password')
  }
  if (value.when === 'charge_below' && !whole(value.percent, 5, 95))
    return t('ups.problems.percent')
  if (value.when === 'after_minutes' && !whole(value.minutes, 1, 720))
    return t('ups.problems.minutes')
  if (local.value && value.serve) {
    if (!value.serveUser.trim()) return t('ups.problems.account')
    if (!value.servePassword && !kept.value.serve) return t('ups.problems.password')
  }
  return null
})

async function apply(): Promise<void> {
  const value = draft.value
  if (!value || problem.value) return
  failure.value = null
  try {
    adopt(await setUps.mutateAsync(toRequest(value)))
  } catch (error) {
    failure.value = problemMessage(error)
  }
}

// What NUT finds on USB.
const found = ref<UpsDevice[] | null>(null)
const scanning = ref(false)

function take(device: UpsDevice): void {
  if (!draft.value || !device.supported) return
  draft.value.driver = device.driver
  draft.value.vendorId = device.vendor_id?.toLowerCase() ?? ''
  draft.value.productId = device.product_id?.toLowerCase() ?? ''
  draft.value.serial = device.serial ?? ''
}
function anyDevice(): void {
  if (!draft.value) return
  draft.value.vendorId = ''
  draft.value.productId = ''
  draft.value.serial = ''
}
async function detect(): Promise<void> {
  scanning.value = true
  failure.value = null
  try {
    found.value = await scanUps()
    // One UPS found: that is the one.
    const usable = found.value.filter((device) => device.supported)
    if (usable.length === 1 && usable[0]) take(usable[0])
  } catch (error) {
    found.value = null
    failure.value = problemMessage(error)
  } finally {
    scanning.value = false
  }
}
const deviceName = (device: UpsDevice) =>
  [device.vendor, device.product].filter(Boolean).join(' ') || t('ups.usb.unnamed')
const deviceId = (vendor?: string | null, product?: string | null) =>
  vendor && product ? `${vendor}:${product}`.toLowerCase() : (vendor ?? '')
const taken = (device: UpsDevice) =>
  draft.value?.vendorId === (device.vendor_id?.toLowerCase() ?? '') &&
  draft.value.productId === (device.product_id?.toLowerCase() ?? '') &&
  draft.value.serial === (device.serial ?? '')

// The installation of NUT, running or done.
const job = computed(() => jobs.list.find((entry) => entry.kind === 'ups.install'))
const installing = computed(() => job.value?.state === 'running')
const failed = computed(() => (job.value?.state === 'failed' ? job.value : null))
watch(
  () => job.value?.state,
  (state, before) => {
    if (before === 'running' && state !== 'running')
      void client.invalidateQueries({ queryKey: ['ups'] })
  },
)
async function install(): Promise<void> {
  failure.value = null
  try {
    jobs.update(await installUps())
  } catch (error) {
    failure.value = problemMessage(error)
  }
}

/** What a device that follows this NAS's UPS is given. */
const forOthers = computed<InfoItem[]>(() => [
  { label: t('ups.serve.address'), value: props.address ?? window.location.hostname, mono: true },
  { label: t('ups.server.port'), value: String(overview.value?.port ?? ''), mono: true },
  { label: t('ups.server.name'), value: overview.value?.name ?? '', mono: true },
  { label: t('ups.server.user'), value: draft.value?.serveUser.trim() ?? '', mono: true },
])

const mono = 'font-mono text-dense'
const number = `w-20 ${mono}`
const hint = 'text-xs text-muted-foreground'
</script>

<!--
  The uninterruptible power supply: where it is (USB, SNMP, another
  device's NUT server), what it says, when the NAS shuts down on battery,
  and whether other devices may follow it through this NAS.
-->
<template>
  <div class="flex min-h-full flex-col">
    <PageHeader :title="t('controlPanel.ups')" :description="t('ups.description')" />
    <PageDescription>
      <p>{{ t('ups.help') }}</p>
      <p>{{ t('ups.helpSources') }}</p>
    </PageDescription>
    <div class="grid max-w-3xl gap-6">
      <Alert v-if="loadError" variant="destructive">{{ problemMessage(loadError) }}</Alert>
      <div v-if="!overview" class="h-48 animate-pulse rounded-lg bg-muted" />

      <PageCard v-else-if="!overview.installed" :title="t('ups.install.title')">
        <div class="grid gap-3">
          <p class="text-muted-foreground">{{ t('ups.install.text') }}</p>
          <p v-if="overview.settings.source" class="text-muted-foreground">
            {{ t('ups.install.waiting') }}
          </p>
          <span v-if="installing" class="flex items-center gap-2 text-muted-foreground">
            <LoaderCircle class="size-4 animate-spin" />
            {{ t('ups.install.running') }}
          </span>
          <div v-else-if="canManage">
            <Button @click="install">{{ t('ups.install.action') }}</Button>
          </div>
          <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
          <template v-if="failed">
            <Alert variant="destructive">{{ t('ups.install.failed') }}</Alert>
            <pre
              v-if="failed.error_detail"
              class="max-h-60 overflow-auto rounded-md bg-zinc-900 p-3 font-mono text-xs whitespace-pre-wrap text-zinc-100"
              >{{ failed.error_detail }}</pre>
          </template>
        </div>
      </PageCard>

      <template v-else-if="draft">
        <UpsStatus v-if="overview.settings.source" :reading="overview.reading" />

        <SettingRow :label="t('ups.source.label')" for="ups-source">
          <NativeSelect
            id="ups-source"
            v-model="draft.kind"
            class="max-w-md"
            :disabled="!canManage"
          >
            <option value="none">{{ t('ups.source.none') }}</option>
            <option value="usb">{{ t('ups.source.usb') }}</option>
            <option value="snmp">{{ t('ups.source.snmp') }}</option>
            <option value="server">{{ t('ups.source.server') }}</option>
            <option v-if="base?.kind === 'simulated'" value="simulated">
              {{ t('ups.source.simulated') }}
            </option>
          </NativeSelect>
          <p :class="hint">{{ t(`ups.source.hints.${draft.kind}`) }}</p>
        </SettingRow>

        <template v-if="draft.kind === 'usb'">
          <SettingRow :label="t('ups.usb.device')">
            <div class="flex flex-wrap items-center gap-3">
              <span v-if="draft.vendorId" :class="mono" data-testid="ups-device">
                {{ deviceId(draft.vendorId, draft.productId) }}
                <template v-if="draft.serial">· {{ draft.serial }}</template>
              </span>
              <span v-else class="text-sm" data-testid="ups-device">{{ t('ups.usb.any') }}</span>
              <Button
                v-if="canManage"
                variant="outline"
                size="sm"
                :disabled="scanning"
                @click="detect"
              >
                <LoaderCircle v-if="scanning" class="animate-spin" />
                <Search v-else />
                {{ t('ups.usb.detect') }}
              </Button>
              <Button
                v-if="canManage && draft.vendorId"
                variant="ghost"
                size="sm"
                @click="anyDevice"
                >{{ t('ups.usb.forget') }}</Button
              >
            </div>
            <p v-if="found?.length === 0" :class="hint" role="status">{{ t('ups.usb.none') }}</p>
            <ul v-else-if="found" class="grid gap-1.5" data-testid="ups-found">
              <li
                v-for="device in found"
                :key="`${device.vendor_id}:${device.product_id}:${device.serial}`"
              >
                <button
                  type="button"
                  class="flex w-full flex-wrap items-baseline gap-x-3 gap-y-0.5 rounded-md border px-3 py-2 text-left text-sm outline-none transition hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60 aria-pressed:border-primary"
                  :disabled="!device.supported"
                  :aria-pressed="taken(device)"
                  @click="take(device)"
                >
                  <span class="font-medium">{{ deviceName(device) }}</span>
                  <span :class="[mono, 'text-muted-foreground']">{{
                    deviceId(device.vendor_id, device.product_id)
                  }}</span>
                  <span :class="[mono, 'ml-auto text-muted-foreground']">{{
                    device.supported
                      ? device.driver
                      : t('ups.usb.unsupported', { driver: device.driver })
                  }}</span>
                </button>
              </li>
            </ul>
            <p :class="hint">{{ t('ups.usb.deviceHint') }}</p>
          </SettingRow>
          <SettingRow :label="t('ups.usb.driver')" for="ups-driver">
            <NativeSelect
              id="ups-driver"
              v-model="draft.driver"
              class="max-w-xs font-mono"
              :disabled="!canManage"
            >
              <option v-for="driver in overview.drivers" :key="driver" :value="driver">
                {{ driver }}
              </option>
            </NativeSelect>
            <p :class="hint">{{ t('ups.usb.driverHint') }}</p>
          </SettingRow>
        </template>

        <template v-else-if="draft.kind === 'snmp'">
          <div
            v-if="!overview.snmp"
            class="flex flex-wrap items-center gap-3 rounded-lg border border-amber-500/50 bg-amber-500/10 px-4 py-3 text-sm"
            role="status"
          >
            <p class="min-w-0 flex-1">
              {{ t(failed ? 'ups.install.failed' : 'errors.ups_snmp_missing') }}
            </p>
            <span v-if="installing" class="flex items-center gap-2 text-muted-foreground">
              <LoaderCircle class="size-4 animate-spin" />
              {{ t('ups.install.running') }}
            </span>
            <Button v-else-if="canManage" size="sm" variant="outline" @click="install">{{
              t('ups.install.action')
            }}</Button>
          </div>
          <SettingRow :label="t('ups.snmp.address')" for="ups-address">
            <Input
              id="ups-address"
              v-model="draft.address"
              :class="['max-w-xs', mono]"
              placeholder="192.168.1.50"
              :disabled="!canManage"
            />
            <p :class="hint">{{ t('ups.snmp.addressHint') }}</p>
          </SettingRow>
          <SettingRow :label="t('ups.snmp.community')" for="ups-community">
            <Input
              id="ups-community"
              v-model="draft.community"
              :class="['max-w-xs', mono]"
              :disabled="!canManage"
            />
          </SettingRow>
          <SettingRow :label="t('ups.snmp.version')" for="ups-version">
            <NativeSelect
              id="ups-version"
              v-model="draft.version"
              class="max-w-32"
              :disabled="!canManage"
            >
              <option value="v1">SNMP v1</option>
              <option value="v2c">SNMP v2c</option>
            </NativeSelect>
          </SettingRow>
        </template>

        <template v-else-if="draft.kind === 'server'">
          <SettingRow :label="t('ups.server.host')" for="ups-host">
            <div class="flex items-center gap-2">
              <Input
                id="ups-host"
                v-model="draft.host"
                :class="['max-w-xs', mono]"
                placeholder="192.168.1.20"
                :disabled="!canManage"
              />
              <span class="text-sm text-muted-foreground">{{ t('ups.server.port') }}</span>
              <Input
                id="ups-port"
                v-model="draft.port"
                inputmode="numeric"
                :class="number"
                :disabled="!canManage"
                :aria-label="t('ups.server.port')"
                :aria-invalid="!whole(draft.port, 1, 65535)"
              />
            </div>
          </SettingRow>
          <SettingRow :label="t('ups.server.name')" for="ups-name">
            <Input
              id="ups-name"
              v-model="draft.ups"
              :class="['max-w-xs', mono]"
              :disabled="!canManage"
            />
            <p :class="hint">{{ t('ups.server.nameHint') }}</p>
          </SettingRow>
          <SettingRow :label="t('ups.server.user')" for="ups-user">
            <Input
              id="ups-user"
              v-model="draft.user"
              :class="['max-w-xs', mono]"
              autocomplete="off"
              :disabled="!canManage"
            />
          </SettingRow>
          <SettingRow :label="t('ups.server.password')" for="ups-password">
            <Input
              id="ups-password"
              v-model="draft.password"
              type="password"
              class="max-w-xs"
              autocomplete="new-password"
              :placeholder="kept.server ? t('ups.passwordKept') : ''"
              :disabled="!canManage"
            />
            <p :class="hint">{{ t('ups.server.accountHint') }}</p>
          </SettingRow>
        </template>

        <template v-if="draft.kind !== 'none'">
          <SettingRow :label="t('ups.shutdown.label')">
            <RadioGroup
              v-model="draft.when"
              :label="t('ups.shutdown.label')"
              :disabled="!canManage"
              class="gap-2.5 pt-1.5"
            >
              <RadioGroupItem value="low_battery">{{
                t('ups.shutdown.lowBattery')
              }}</RadioGroupItem>
              <RadioGroupItem value="charge_below" :disabled="!local">
                <i18n-t
                  keypath="ups.shutdown.chargeBelow"
                  tag="span"
                  class="flex items-center gap-2"
                >
                  <template #percent>
                    <Input
                      id="ups-percent"
                      v-model="draft.percent"
                      inputmode="numeric"
                      :class="number"
                      :disabled="!canManage || draft.when !== 'charge_below'"
                      :aria-label="t('ups.shutdown.percent')"
                      :aria-invalid="draft.when === 'charge_below' && !whole(draft.percent, 5, 95)"
                    />
                  </template>
                </i18n-t>
              </RadioGroupItem>
              <RadioGroupItem value="after_minutes">
                <i18n-t
                  keypath="ups.shutdown.afterMinutes"
                  tag="span"
                  class="flex items-center gap-2"
                >
                  <template #minutes>
                    <Input
                      id="ups-minutes"
                      v-model="draft.minutes"
                      inputmode="numeric"
                      :class="number"
                      :disabled="!canManage || draft.when !== 'after_minutes'"
                      :aria-label="t('ups.shutdown.minutes')"
                      :aria-invalid="
                        draft.when === 'after_minutes' && !whole(draft.minutes, 1, 720)
                      "
                    />
                  </template>
                </i18n-t>
              </RadioGroupItem>
            </RadioGroup>
            <p :class="hint">
              {{ t(local ? 'ups.shutdown.hint' : 'ups.shutdown.hintServer') }}
            </p>
          </SettingRow>

          <SettingRow v-if="local" :label="t('ups.cut.label')" for="ups-cut">
            <label class="flex items-center gap-3 pt-1 text-sm">
              <Switch id="ups-cut" v-model="draft.cutPower" :disabled="!canManage" />
              {{ t('ups.cut.text') }}
            </label>
            <p :class="hint">{{ t('ups.cut.hint') }}</p>
          </SettingRow>

          <SettingRow v-if="local" :label="t('ups.serve.label')" for="ups-serve">
            <label class="flex items-center gap-3 pt-1 text-sm">
              <Switch id="ups-serve" v-model="draft.serve" :disabled="!canManage" />
              {{ t('ups.serve.text') }}
            </label>
            <p :class="hint">{{ t('ups.serve.hint') }}</p>
            <template v-if="draft.serve">
              <div class="flex flex-wrap items-center gap-2 pt-1">
                <Input
                  id="ups-serve-user"
                  v-model="draft.serveUser"
                  :class="['max-w-40', mono]"
                  autocomplete="off"
                  :disabled="!canManage"
                  :aria-label="t('ups.server.user')"
                />
                <Input
                  id="ups-serve-password"
                  v-model="draft.servePassword"
                  type="password"
                  class="max-w-48"
                  autocomplete="new-password"
                  :placeholder="kept.serve ? t('ups.passwordKept') : t('ups.server.password')"
                  :disabled="!canManage"
                  :aria-label="t('ups.server.password')"
                />
              </div>
              <p :class="hint">{{ t('ups.serve.accountHint') }}</p>
              <div class="grid gap-2 rounded-lg bg-muted/60 p-3" data-testid="ups-for-others">
                <p class="text-sm font-medium">{{ t('ups.serve.there') }}</p>
                <InfoList :items="forOthers" />
                <p :class="hint">{{ t('ups.serve.firewall', { port: overview.port }) }}</p>
              </div>
            </template>
          </SettingRow>
        </template>
      </template>
    </div>
    <div class="grow" aria-hidden="true" />
    <PageFooter
      v-if="draft && canManage && overview?.installed"
      :dirty="dirty"
      :saving="setUps.isPending.value"
      :valid="problem === null"
      :error="failure ?? (dirty ? problem : null)"
      @reset="reset()"
      @apply="apply()"
    />
  </div>
</template>
