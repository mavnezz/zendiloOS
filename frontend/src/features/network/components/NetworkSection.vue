<script setup lang="ts">
import { Cable, Network, Pencil, Wifi } from 'lucide-vue-next'
import { computed, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NetworkInterface } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'

import { useNetwork, useNetworkActions, useNetworkSetup } from '../api'
import InterfaceDialog from './InterfaceDialog.vue'

const i18n = useI18n()
const { t } = i18n
const { data: network, error } = useNetwork()
/** Whether the signed-in account may change the network (`system.manage`). */
const props = defineProps<{ canManage: boolean }>()

const { data: setup } = useNetworkSetup()
const actions = useNetworkActions()

const CHANGEABLE = ['ethernet', 'bridge', 'bond', 'vlan']
const changeable = (item: NetworkInterface) =>
  setup.value?.managed === true &&
  props.canManage &&
  !item.member_of &&
  CHANGEABLE.includes(item.kind)

const editing = ref<NetworkInterface | null>(null)
const dialogOpen = ref(false)
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

/** Link-local addresses only matter to experts; they come last anyway. */
const shown = (item: NetworkInterface) => item.addresses.filter((a) => a.scope !== 'link')
const linkLocal = (item: NetworkInterface) => item.addresses.filter((a) => a.scope === 'link')
</script>

<!-- How the machine is connected. Read only for now. -->
<template>
  <div class="grid gap-6">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <div
      v-if="setup?.pending"
      class="grid gap-2 rounded-xl border border-amber-500/50 bg-amber-500/10 p-4 text-sm"
      role="status"
    >
      <p class="font-medium">
        {{ t('network.pending.title', { name: setup.pending.interface }) }}
      </p>
      <p>{{ t('network.pending.hint', { seconds: secondsLeft }) }}</p>
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
                {{ item.mtu }}
              </p>
            </div>
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
      <p v-if="setup && !setup.managed" class="text-xs text-muted-foreground">
        {{ t('network.readOnly') }}
      </p>
    </template>
    <InterfaceDialog
      v-model:open="dialogOpen"
      :item="editing"
      :network="network ?? null"
      @applied="(address) => (newAddress = address)"
    />
  </div>
</template>
