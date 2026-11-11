<script setup lang="ts">
import { TriangleAlert } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { InterfaceSettingsInput, NetworkInfo, NetworkInterface } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useNetworkActions } from '../api'

/** The interface to change, and the network around it (to start from). */
const props = defineProps<{ item: NetworkInterface | null; network: NetworkInfo | null }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ applied: [address: string | null] }>()

const { t } = useI18n()
const actions = useNetworkActions()

interface Form {
  mode: 'dhcp' | 'static' | 'off'
  address: string
  prefix: string
  gateway: string
  ipv6: 'automatic' | 'off'
  /** Separated by commas or spaces. */
  dns: string
}

const form = ref<Form>({
  mode: 'dhcp',
  address: '',
  prefix: '24',
  gateway: '',
  ipv6: 'automatic',
  dns: '',
})
const failure = ref<string | null>(null)
const saving = ref(false)

/** Down or never set up: the dialog switches it on. */
const switchingOn = computed(() => !!props.item && props.item.addresses.length === 0)
/** Neither IPv4 nor IPv6: the interface is switched off. */
const off = computed(() => form.value.mode === 'off' && form.value.ipv6 === 'off')
/**
 * Switching a card off is asked again: if the NAS is reached through it, it
 * is not any more until the old settings return.
 */
const confirmingOff = ref(false)
watch(form, () => (confirmingOff.value = false), { deep: true })

// Starts from how the interface is set up now; one without any address
// starts with DHCP and IPv6, to be switched on.
watch(open, (isOpen) => {
  if (!isOpen || !props.item) return
  failure.value = null
  const item = props.item
  const v4 = item.addresses.find((a) => a.scope === 'global' && !a.address.includes(':'))
  const route = props.network?.default_routes.find(
    (r) => r.interface === item.name && !r.gateway.includes(':'),
  )
  const servers = (props.network?.name_servers ?? []).filter((s) => s !== '127.0.0.53')
  const v6 = item.addresses.some((a) => a.address.includes(':'))
  form.value = {
    mode: v4 && !v4.dynamic ? 'static' : 'dhcp',
    address: v4?.address ?? '',
    prefix: String(v4?.prefix ?? 24),
    gateway: route?.gateway ?? '',
    // Without any IPv6 address, not even link-local, IPv6 is off.
    ipv6: v6 || item.addresses.length === 0 ? 'automatic' : 'off',
    dns: v4 && !v4.dynamic ? servers.join(', ') : '',
  }
})

function ipv4(f: Form): InterfaceSettingsInput['ipv4'] {
  if (f.mode === 'dhcp') return { mode: 'dhcp' }
  if (f.mode === 'off') return { mode: 'off' }
  return {
    mode: 'static',
    address: f.address.trim(),
    prefix: Number(f.prefix),
    gateway: f.gateway.trim() || null,
  }
}

function body(): InterfaceSettingsInput {
  const f = form.value
  // A card switched off has no name servers.
  const dns = off.value
    ? []
    : f.dns
        .split(/[\s,;]+/)
        .map((s) => s.trim())
        .filter(Boolean)
  return { ipv4: ipv4(f), ipv6: f.ipv6, dns }
}

async function send(settings: InterfaceSettingsInput): Promise<void> {
  const item = props.item
  if (!item) return
  failure.value = null
  saving.value = true
  try {
    await actions.configure(item.name, settings)
    open.value = false
    emit('applied', settings.ipv4.mode === 'static' ? settings.ipv4.address : null)
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}

function apply(): void {
  if (off.value && !confirmingOff.value) {
    confirmingOff.value = true
    return
  }
  void send(body())
}

const select = 'h-9 w-full rounded-md border bg-background px-3 text-sm'
</script>

<!-- Changes one interface, switches it on or off; the old settings return unless confirmed. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="
      switchingOn
        ? t('network.change.titleOn', { name: item?.name ?? '' })
        : t('network.change.title', { name: item?.name ?? '' })
    "
    :description="t('network.change.hint')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-3" @submit.prevent="apply">
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      <div class="grid gap-1.5">
        <Label for="net-mode">IPv4</Label>
        <select id="net-mode" v-model="form.mode" :class="select">
          <option value="dhcp">{{ t('network.change.dhcp') }}</option>
          <option value="static">{{ t('network.change.static') }}</option>
          <option value="off">{{ t('network.change.ipv4Off') }}</option>
        </select>
      </div>
      <template v-if="form.mode === 'static'">
        <div class="grid grid-cols-[1fr_5rem] gap-2">
          <div class="grid gap-1.5">
            <Label for="net-address">{{ t('network.change.address') }}</Label>
            <Input id="net-address" v-model="form.address" placeholder="192.168.1.10" required />
          </div>
          <div class="grid gap-1.5">
            <Label for="net-prefix">{{ t('network.change.prefix') }}</Label>
            <Input id="net-prefix" v-model="form.prefix" type="number" min="1" max="32" required />
          </div>
        </div>
        <div class="grid gap-1.5">
          <Label for="net-gateway">{{ t('network.gateway') }}</Label>
          <Input id="net-gateway" v-model="form.gateway" placeholder="192.168.1.1" />
        </div>
      </template>
      <div v-if="!off" class="grid gap-1.5">
        <Label for="net-dns">{{ t('network.nameServers') }}</Label>
        <Input id="net-dns" v-model="form.dns" :placeholder="t('network.change.dnsFromDhcp')" />
        <p class="text-xs text-muted-foreground">{{ t('network.change.dnsHint') }}</p>
      </div>
      <div class="grid gap-1.5">
        <Label for="net-ipv6">IPv6</Label>
        <select id="net-ipv6" v-model="form.ipv6" :class="select">
          <option value="automatic">{{ t('network.change.ipv6Automatic') }}</option>
          <option value="off">{{ t('network.change.ipv6Off') }}</option>
        </select>
      </div>
      <p v-if="off && !confirmingOff" class="text-sm text-muted-foreground" role="status">
        {{ t('network.change.switchedOff') }}
      </p>
      <Alert v-if="confirmingOff" variant="destructive" role="alert">
        <TriangleAlert />
        <div class="grid gap-1">
          <p class="font-medium">
            {{ t('network.change.offQuestion', { name: item?.name ?? '' }) }}
          </p>
          <p>{{ t('network.change.offWarning') }}</p>
        </div>
      </Alert>
      <div class="flex justify-end gap-2">
        <template v-if="confirmingOff">
          <Button type="button" variant="outline" :disabled="saving" @click="confirmingOff = false">
            {{ t('desktop.back') }}
          </Button>
          <Button type="submit" variant="destructive" :disabled="saving">
            {{ t('network.change.switchOff') }}
          </Button>
        </template>
        <Button v-else type="submit" :disabled="saving">{{ t('network.change.apply') }}</Button>
      </div>
    </form>
  </ModalDialog>
</template>
