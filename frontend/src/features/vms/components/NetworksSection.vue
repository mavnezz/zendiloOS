<script setup lang="ts">
import {
  Cable,
  EthernetPort,
  Globe,
  Network,
  Plus,
  Trash2,
  TriangleAlert,
  Waypoints,
} from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmPlugin } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useOpenApp } from '@/shared/lib/window-section'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useInternalNetworks } from '../api'
import {
  interfaceNameValid,
  nextInternalName,
  vmNetworks,
  type NetworkEntry,
} from '../lib/networks'
import { NETWORK_KINDS, type NetworkKind } from '../lib/vms'

/** Every network a VM can use, by kind, with the VMs whose cards are on each. */
const props = defineProps<{ plugin: VmPlugin; vms: VirtualMachine[] }>()

const { t } = useI18n()
const openApp = useOpenApp()
const internal = useInternalNetworks()

const networks = computed(() => vmNetworks(props.plugin, props.vms))
const ICONS = { user: Globe, bridge: Network, direct: EthernetPort, internal: Waypoints }

const failure = ref<string | null>(null)
const adding = ref(false)
const name = ref('')
const removing = ref<string | null>(null)
const pending = ref(false)

/** Names taken by the NAS's networks and cards. */
const taken = computed(() => [
  ...props.plugin.bridges,
  ...props.plugin.internal_networks,
  ...props.plugin.cards,
])
watch(adding, (open) => {
  if (open) name.value = nextInternalName(taken.value)
})
const nameValid = computed(
  () => interfaceNameValid(name.value) && !taken.value.includes(name.value),
)

function entryName(entry: NetworkEntry): string {
  return entry.device === null
    ? t('vms.networks.kinds.user')
    : t(`vms.networks.on.${entry.kind}`, { device: entry.device })
}

function toNetworkPage(): void {
  openApp?.('settings', 'network')
}

async function add(): Promise<void> {
  if (!nameValid.value) return
  pending.value = true
  failure.value = null
  try {
    await internal.add(name.value)
    adding.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
    adding.value = false
  } finally {
    pending.value = false
  }
}

async function remove(): Promise<void> {
  const network = removing.value
  if (!network) return
  failure.value = null
  try {
    await internal.remove(network)
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

const empty: Record<Exclude<NetworkKind, 'user'>, string> = {
  bridge: 'vms.networks.noBridges',
  direct: 'vms.networks.noCards',
  internal: 'vms.networks.noInternal',
}
</script>

<!-- The networks of VMs: through the NAS, host bridges, directly on a card, internal; and who is on each. -->
<template>
  <div class="grid content-start gap-6">
    <header class="grid gap-0.5">
      <h2 class="text-xl font-semibold">{{ t('vms.sections.networks') }}</h2>
      <p class="text-sm text-muted-foreground">{{ t('vms.networks.description') }}</p>
    </header>

    <Alert v-if="failure" variant="destructive" data-testid="vm-networks-failure">{{
      failure
    }}</Alert>

    <section
      v-for="kind in NETWORK_KINDS"
      :key="kind"
      class="grid gap-3 rounded-xl border bg-card p-4"
      :data-kind="kind"
      :aria-labelledby="`vm-networks-${kind}`"
    >
      <div class="flex flex-wrap items-start gap-3">
        <span
          class="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground"
          aria-hidden="true"
        >
          <component :is="ICONS[kind]" class="size-5" />
        </span>
        <div class="grid min-w-0 flex-1 gap-0.5">
          <h3 :id="`vm-networks-${kind}`" class="font-medium">
            {{ t(`vms.networks.kinds.${kind}`) }}
          </h3>
          <p class="text-sm text-muted-foreground">{{ t(`vms.networks.means.${kind}`) }}</p>
        </div>
        <Button
          v-if="kind === 'bridge' && openApp"
          variant="outline"
          size="sm"
          data-testid="vm-networks-bridge"
          @click="toNetworkPage"
        >
          <Cable />
          {{ t('vms.networks.addBridge') }}
        </Button>
        <Button
          v-if="kind === 'internal'"
          size="sm"
          data-testid="vm-networks-add-internal"
          @click="adding = true"
        >
          <Plus />
          {{ t('vms.networks.addInternal') }}
        </Button>
      </div>

      <Alert v-if="kind === 'direct'" data-testid="vm-networks-direct-warning">
        <TriangleAlert class="text-warning" />
        {{ t('vms.networks.directWarning') }}
      </Alert>

      <p
        v-if="kind !== 'user' && !networks[kind].some((entry) => entry.present)"
        class="text-sm text-muted-foreground"
      >
        {{ t(empty[kind]) }}
      </p>
      <ul v-if="networks[kind].length" class="grid gap-1.5">
        <li
          v-for="entry in networks[kind]"
          :key="entry.device ?? kind"
          :data-network="entry.device ?? kind"
          class="flex flex-wrap items-center gap-2 rounded-lg border px-3 py-2"
        >
          <span class="min-w-0 flex-1 truncate text-sm font-medium">
            {{ entryName(entry) }}
          </span>
          <Badge v-if="kind === 'user'" variant="secondary">{{ t('vms.networks.always') }}</Badge>
          <Badge v-if="!entry.present" variant="warning">{{ t('vms.networks.gone') }}</Badge>
          <span class="flex flex-wrap gap-1" :aria-label="t('vms.networks.vms')">
            <Badge v-for="vm in entry.vms" :key="vm.id" variant="outline">{{ vm.name }}</Badge>
            <span v-if="!entry.vms.length" class="text-xs text-muted-foreground">{{
              t('vms.networks.noVms')
            }}</span>
          </span>
          <Button
            v-if="kind === 'internal' && entry.present"
            variant="ghost"
            size="icon"
            :disabled="entry.vms.length > 0"
            :aria-label="t('vms.networks.removeInternal', { name: entry.device })"
            @click="removing = entry.device"
          >
            <Trash2 />
          </Button>
        </li>
      </ul>
    </section>

    <ModalDialog
      v-model:open="adding"
      :title="t('vms.networks.internalDialog.title')"
      :description="t('vms.networks.internalDialog.description')"
      :close-label="t('desktop.close')"
    >
      <form class="grid gap-4" @submit.prevent="add">
        <div class="grid gap-2">
          <Label for="vm-internal-name">{{ t('vms.networks.internalDialog.name') }}</Label>
          <Input
            id="vm-internal-name"
            v-model="name"
            maxlength="15"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="!nameValid"
          />
          <p class="text-xs text-muted-foreground">
            {{ t('vms.networks.internalDialog.nameHint') }}
          </p>
        </div>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" @click="adding = false">
            {{ t('desktop.cancel') }}
          </Button>
          <Button type="submit" :disabled="pending || !nameValid">
            {{ t('vms.networks.internalDialog.submit') }}
          </Button>
        </div>
      </form>
    </ModalDialog>
    <ConfirmDialog
      :open="Boolean(removing)"
      :title="t('vms.networks.removeDialog.title', { name: removing ?? '' })"
      :description="t('vms.networks.removeDialog.description')"
      :confirm-label="t('vms.networks.removeDialog.submit')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (removing = null)"
      @confirm="remove"
    />
  </div>
</template>
