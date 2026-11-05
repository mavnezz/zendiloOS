<script setup lang="ts">
import { Cable, Network, Wifi } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import type { NetworkInterface } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'

import { useNetwork } from '../api'

const i18n = useI18n()
const { t } = i18n
const { data: network, error } = useNetwork()

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
      <p class="text-xs text-muted-foreground">{{ t('network.readOnly') }}</p>
    </template>
  </div>
</template>
