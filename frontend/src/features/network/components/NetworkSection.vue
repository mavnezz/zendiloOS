<script setup lang="ts">
import { Cable, LoaderCircle, Network, Pencil, Plus, Trash2, Users, Wifi } from 'lucide-vue-next'
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuTrigger,
} from 'reka-ui'
import { computed, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NetworkDevice, NetworkInterface, PlannedInterface } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'

import { useNetwork, useNetworkActions, useNetworkSetup } from '../api'
import { type DeviceKind, composition, describeSettings } from '../lib/devices'
import DeviceDialog from './DeviceDialog.vue'
import InterfaceDialog from './InterfaceDialog.vue'
import RemoveDeviceDialog from './RemoveDeviceDialog.vue'

const i18n = useI18n()
const { t } = i18n
const { data: network, error } = useNetwork()
/** Whether the signed-in account may change the network (`system.manage`). */
const props = defineProps<{ canManage: boolean }>()

const { data: setup, refetch: refetchSetup } = useNetworkSetup()
const actions = useNetworkActions()
const jobs = useJobsStore()

const CHANGEABLE = ['ethernet', 'bridge', 'bond', 'vlan']
const changeable = (item: NetworkInterface) =>
  setup.value?.managed === true &&
  props.canManage &&
  !item.member_of &&
  CHANGEABLE.includes(item.kind)

const editing = ref<NetworkInterface | null>(null)
const dialogOpen = ref(false)

/** The bridges, bonds and VLANs zendiloOS made: these change and go. */
const devices = computed(() => setup.value?.devices ?? [])
const madeHere = (item: NetworkInterface) => devices.value.find((d) => d.name === item.name)
const canAdd = computed(
  () => setup.value?.managed === true && props.canManage && !setup.value.pending,
)
const ADDABLE: DeviceKind[] = ['bridge', 'bond', 'vlan']
const deviceKind = ref<DeviceKind | null>(null)
const deviceEditing = ref<NetworkDevice | null>(null)
const deviceOpen = ref(false)
const removing = ref<NetworkDevice | null>(null)
const removeOpen = ref(false)

function add(kind: DeviceKind): void {
  deviceKind.value = kind
  deviceEditing.value = null
  deviceOpen.value = true
}

function changeMembers(device: NetworkDevice): void {
  deviceKind.value = null
  deviceEditing.value = device
  deviceOpen.value = true
}

function remove(device: NetworkDevice): void {
  removing.value = device
  removeOpen.value = true
}
/** The static address just applied: the NAS is confirmed from there. */
const newAddress = ref<string | null>(null)
const failure = ref<string | null>(null)

// A clock for the countdown.
const now = ref(Date.now())
const ticking = setInterval(() => (now.value = Date.now()), 1000)
onUnmounted(() => clearInterval(ticking))
const secondsLeft = computed(() => {
  const at = setup.value?.pending?.revert_at
  return at ? Math.max(0, Math.round(at - now.value / 1000)) : 0
})
const elsewhere = computed(
  () => newAddress.value !== null && newAddress.value !== window.location.hostname,
)

/** The latest takeover from ifupdown, running or done. */
const takeoverJob = computed(() => jobs.list.find((job) => job.kind === 'network.take_over'))
const takingOver = computed(() => takeoverJob.value?.state === 'running')
const starting = ref(false)
const takeover = computed(() => (setup.value?.manager === 'ifupdown' ? setup.value.takeover : null))
const canTakeOver = computed(
  () =>
    props.canManage &&
    !!takeover.value &&
    takeover.value.refused.length === 0 &&
    takeover.value.ifupdown_running &&
    !takingOver.value &&
    !starting.value,
)

// Once a takeover ends, what manages the network now; why, if it failed.
watch(
  () => takeoverJob.value?.state,
  (state, before) => {
    if (before !== 'running' || state === 'running') return
    void refetchSetup()
    const job = takeoverJob.value
    if (state === 'failed' && job) failure.value = job.error_detail ?? job.error_code ?? null
  },
)

async function takeOver(): Promise<void> {
  failure.value = null
  starting.value = true
  try {
    jobs.update(await actions.takeOver())
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    starting.value = false
  }
}

function change(item: NetworkInterface): void {
  editing.value = item
  dialogOpen.value = true
}

async function confirm(): Promise<void> {
  failure.value = null
  try {
    await actions.confirm()
    newAddress.value = null
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

const KNOWN_KINDS = ['ethernet', 'wireless', 'bridge', 'bond', 'vlan']

function kind(item: NetworkInterface): string {
  return KNOWN_KINDS.includes(item.kind) ? t(`network.kinds.${item.kind}`) : item.kind
}

function icon(item: NetworkInterface) {
  if (item.kind === 'wireless') return Wifi
  if (item.kind === 'ethernet') return Cable
  return Network
}

/** How the takeover sets an interface up, in a line. */
const planned = (item: PlannedInterface) => describeSettings(item, t)

/** What the pending change is, in its title and hint. */
const pendingTitle = computed(() => {
  const pending = setup.value?.pending
  if (!pending) return ''
  if (pending.takeover) return t('network.pending.takeoverTitle')
  if (pending.device)
    return t(`network.pending.deviceTitle.${pending.device}`, { name: pending.interface })
  return t('network.pending.title', { name: pending.interface })
})
const pendingHint = computed(() => {
  const pending = setup.value?.pending
  const seconds = secondsLeft.value
  if (pending?.takeover) return t('network.pending.takeoverHint', { seconds })
  if (pending?.device) return t('network.pending.deviceHint', { seconds })
  return t('network.pending.hint', { seconds })
})

/** Link-local addresses only matter to experts; they come last anyway. */
const shown = (item: NetworkInterface) => item.addresses.filter((a) => a.scope !== 'link')
const linkLocal = (item: NetworkInterface) => item.addresses.filter((a) => a.scope === 'link')
</script>

<!-- How the machine is connected, changed where zendiloOS manages it. -->
<template>
  <div class="grid gap-6">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <div
      v-if="setup?.pending"
      class="grid gap-2 rounded-xl border border-amber-500/50 bg-amber-500/10 p-4 text-sm"
      role="status"
    >
      <p class="font-medium">{{ pendingTitle }}</p>
      <p>{{ pendingHint }}</p>
      <p v-if="elsewhere">
        {{ t('network.pending.elsewhere') }}
        <a class="font-medium underline" :href="`https://${newAddress}/`"
          >https://{{ newAddress }}/</a
        >
      </p>
      <div v-if="canManage" class="flex justify-end">
        <Button size="sm" @click="confirm">{{ t('network.pending.confirm') }}</Button>
      </div>
    </div>
    <template v-else-if="network">
      <section
        v-if="takeover"
        class="grid gap-3 rounded-xl border bg-card p-4 text-sm"
        aria-labelledby="network-takeover"
      >
        <h3 id="network-takeover" class="font-medium">{{ t('network.takeover.title') }}</h3>
        <p class="text-muted-foreground">{{ t('network.takeover.hint') }}</p>
        <div v-if="takeover.interfaces.length" class="grid gap-1">
          <p class="font-medium">{{ t('network.takeover.interfaces') }}</p>
          <ul class="grid gap-1">
            <li v-for="item in takeover.interfaces" :key="item.name" class="flex flex-wrap gap-2">
              <span class="font-mono">{{ item.name }}</span>
              <span class="text-muted-foreground">{{ planned(item) }}</span>
            </li>
          </ul>
          <p class="text-xs text-muted-foreground">{{ t('network.takeover.others') }}</p>
        </div>
        <div v-if="takeover.refused.length" class="grid gap-1" role="alert">
          <p>{{ t('network.takeover.refused') }}</p>
          <ul class="grid gap-1">
            <li v-for="(item, index) in takeover.refused" :key="index">
              <span class="font-mono">{{ item.place }}</span
              ><span class="text-muted-foreground">: {{ item.reason }}</span>
            </li>
          </ul>
        </div>
        <p v-else-if="!takeover.ifupdown_running" role="alert">
          {{ t('network.takeover.notRunning') }}
        </p>
        <p v-if="takeover.installs_resolved" class="text-xs text-muted-foreground">
          {{ t('network.takeover.resolved') }}
        </p>
        <div v-if="canManage" class="flex items-center justify-end gap-3">
          <span
            v-if="takingOver"
            class="flex items-center gap-2 text-muted-foreground"
            role="status"
          >
            <LoaderCircle class="size-4 animate-spin" />
            {{
              takeoverJob?.step === 'packages'
                ? t('network.takeover.installing')
                : t('network.takeover.working')
            }}
          </span>
          <Button size="sm" :disabled="!canTakeOver" @click="takeOver">
            {{ t('network.takeover.button') }}
          </Button>
        </div>
      </section>

      <div v-if="canAdd" class="flex justify-end">
        <DropdownMenuRoot :modal="false">
          <DropdownMenuTrigger as-child>
            <Button size="sm" variant="outline"><Plus />{{ t('network.add.button') }}</Button>
          </DropdownMenuTrigger>
          <DropdownMenuPortal>
            <DropdownMenuContent
              align="end"
              :side-offset="4"
              class="z-[1100] min-w-40 rounded-xl border bg-popover p-1 text-popover-foreground shadow-lg"
            >
              <DropdownMenuItem
                v-for="addable in ADDABLE"
                :key="addable"
                class="flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-sm outline-none select-none data-[highlighted]:bg-accent"
                @select="add(addable)"
              >
                {{ t(`network.add.${addable}`) }}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenuPortal>
        </DropdownMenuRoot>
      </div>

      <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
        <dt class="text-muted-foreground">{{ t('network.gateway') }}</dt>
        <dd>
          <template v-if="network.default_routes.length">
            <span v-for="(route, index) in network.default_routes" :key="index" class="block">
              {{ route.gateway }}
              <span class="text-muted-foreground">({{ route.interface }})</span>
            </span>
          </template>
          <span v-else class="text-muted-foreground">{{ t('network.none') }}</span>
        </dd>
        <dt class="text-muted-foreground">{{ t('network.nameServers') }}</dt>
        <dd>
          {{ network.name_servers.length ? network.name_servers.join(', ') : t('network.none') }}
        </dd>
      </dl>

      <div class="grid gap-3 @3xl:grid-cols-2">
        <section
          v-for="item in network.interfaces"
          :key="item.name"
          class="grid gap-2 rounded-xl border bg-card p-4"
        >
          <header class="flex items-start gap-3">
            <component :is="icon(item)" class="mt-0.5 size-5 shrink-0 text-primary" />
            <div class="min-w-0 flex-1">
              <h3 class="truncate font-medium">{{ item.name }}</h3>
              <p class="truncate text-xs text-muted-foreground">
                {{ kind(item) }}<template v-if="item.mac"> · {{ item.mac }}</template> · MTU
                {{ item.mtu
                }}<template v-if="madeHere(item)"> · {{ t('network.devices.madeHere') }}</template>
              </p>
            </div>
            <Button
              v-if="canAdd && madeHere(item) && madeHere(item)?.kind !== 'vlan'"
              variant="ghost"
              size="icon"
              :title="t('network.devices.change')"
              :aria-label="t('network.devices.change')"
              @click="changeMembers(madeHere(item)!)"
            >
              <Users />
            </Button>
            <Button
              v-if="canAdd && madeHere(item)"
              variant="ghost"
              size="icon"
              :title="t('network.devices.remove')"
              :aria-label="t('network.devices.remove')"
              @click="remove(madeHere(item)!)"
            >
              <Trash2 />
            </Button>
            <Button
              v-if="changeable(item)"
              variant="ghost"
              size="icon"
              :title="t('network.change.button')"
              :aria-label="t('network.change.button')"
              :disabled="!!setup?.pending"
              @click="change(item)"
            >
              <Pencil />
            </Button>
            <Badge
              :class="
                cn(
                  item.state === 'up'
                    ? 'bg-emerald-600/15 text-emerald-700 dark:text-emerald-300'
                    : 'bg-muted text-muted-foreground',
                )
              "
            >
              {{ t(`network.states.${item.state}`) }}
            </Badge>
          </header>
          <p
            v-if="network && composition(item, network, devices, t)"
            class="text-sm text-muted-foreground"
          >
            {{ composition(item, network, devices, t) }}
          </p>
          <p v-if="item.member_of" class="text-sm text-muted-foreground">
            {{ t('network.memberOf', { name: item.member_of }) }}
          </p>
          <ul v-else-if="shown(item).length" class="grid gap-1 text-sm">
            <li v-for="address in shown(item)" :key="address.address" class="flex gap-2">
              <span class="font-mono">{{ address.address }}/{{ address.prefix }}</span>
              <span class="text-muted-foreground">
                {{ address.dynamic ? t('network.dynamic') : t('network.fixed') }}
              </span>
            </li>
          </ul>
          <p v-else class="text-sm text-muted-foreground">{{ t('network.noAddress') }}</p>
          <p v-if="linkLocal(item).length" class="truncate text-xs text-muted-foreground">
            {{ t('network.linkLocal') }}:
            {{
              linkLocal(item)
                .map((a) => a.address)
                .join(', ')
            }}
          </p>
        </section>
      </div>
      <p v-if="setup?.manager === 'network_manager'" class="text-xs text-muted-foreground">
        {{ t('network.networkManager') }}
      </p>
      <p v-else-if="setup?.manager === 'unknown'" class="text-xs text-muted-foreground">
        {{ t('network.readOnly') }}
      </p>
    </template>
    <DeviceDialog
      v-model:open="deviceOpen"
      :kind="deviceKind"
      :editing="deviceEditing"
      :network="network ?? null"
      :devices="devices"
    />
    <RemoveDeviceDialog v-model:open="removeOpen" :device="removing" />
    <InterfaceDialog
      v-model:open="dialogOpen"
      :item="editing"
      :network="network ?? null"
      @applied="(address) => (newAddress = address)"
    />
  </div>
</template>
