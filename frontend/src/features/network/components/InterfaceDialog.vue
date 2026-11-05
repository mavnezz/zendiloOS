<script setup lang="ts">
import { ref, watch } from 'vue'
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
  mode: 'dhcp' | 'static'
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

// Starts from how the interface is set up now.
watch(open, (isOpen) => {
  if (!isOpen || !props.item) return
  failure.value = null
  const item = props.item
  const v4 = item.addresses.find((a) => a.scope === 'global' && !a.address.includes(':'))
  const route = props.network?.default_routes.find(
    (r) => r.interface === item.name && !r.gateway.includes(':'),
  )
  const servers = (props.network?.name_servers ?? []).filter((s) => s !== '127.0.0.53')
  form.value = {
    mode: v4 && !v4.dynamic ? 'static' : 'dhcp',
    address: v4?.address ?? '',
    prefix: String(v4?.prefix ?? 24),
    gateway: route?.gateway ?? '',
    // Without any IPv6 address, not even link-local, IPv6 is off.
    ipv6: item.addresses.some((a) => a.address.includes(':')) ? 'automatic' : 'off',
    dns: v4 && !v4.dynamic ? servers.join(', ') : '',
  }
})

function body(): InterfaceSettingsInput {
  const f = form.value
  const dns = f.dns
    .split(/[\s,;]+/)
    .map((s) => s.trim())
    .filter(Boolean)
  return {
    ipv4:
      f.mode === 'dhcp'
        ? { mode: 'dhcp' }
        : {
            mode: 'static',
            address: f.address.trim(),
            prefix: Number(f.prefix),
            gateway: f.gateway.trim() || null,
          },
    ipv6: f.ipv6,
    dns,
  }
}

async function apply(): Promise<void> {
  const item = props.item
  if (!item) return
  failure.value = null
  saving.value = true
  try {
    await actions.configure(item.name, body())
    open.value = false
    emit('applied', form.value.mode === 'static' ? form.value.address.trim() : null)
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}

const select = 'h-9 w-full rounded-md border bg-background px-3 text-sm'
</script>

<!-- Changes one interface; the old settings return unless confirmed. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('network.change.title', { name: item?.name ?? '' })"
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
      <div class="grid gap-1.5">
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
      <div class="flex justify-end">
        <Button type="submit" :disabled="saving">{{ t('network.change.apply') }}</Button>
      </div>
    </form>
  </ModalDialog>
</template>
