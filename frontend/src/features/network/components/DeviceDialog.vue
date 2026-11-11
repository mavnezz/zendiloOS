<script setup lang="ts">
import { LoaderCircle } from 'lucide-vue-next'
import { computed, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type {
  BondMode,
  DevicePlan,
  DevicePlanInput,
  InterfaceSettingsInput,
  NetworkDevice,
  NetworkInfo,
  NewDeviceInput,
} from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'

import { useNetworkActions } from '../api'
import {
  type DeviceKind,
  NAME_MAX,
  candidates,
  describeFate,
  freeName,
  parents,
  vlanName,
} from '../lib/devices'

/**
 * A bridge, bond or VLAN to make (`kind`), or one zendiloOS made whose
 * members change (`editing`); the network around it to choose from.
 */
const props = defineProps<{
  kind: DeviceKind | null
  editing: NetworkDevice | null
  network: NetworkInfo | null
  devices: NetworkDevice[]
}>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ done: [] }>()

const { t } = useI18n()
const actions = useNetworkActions()

const MODES: BondMode[] = ['active_backup', 'lacp', 'balance_alb']

interface Form {
  name: string
  /** In the order chosen: the first with settings gives them. */
  members: string[]
  stp: boolean
  mode: BondMode
  parent: string
  id: string
  ipv4: 'off' | 'dhcp' | 'static'
  address: string
  prefix: string
  gateway: string
  ipv6: 'automatic' | 'off'
}

const form = ref<Form>(blank())
const plan = ref<DevicePlan | null>(null)
const planProblem = ref<string | null>(null)
const planning = ref(false)
const failure = ref<string | null>(null)
const saving = ref(false)

/** What is made, or changed. */
const kind = computed<DeviceKind>(() => props.editing?.kind ?? props.kind ?? 'bridge')

function blank(): Form {
  return {
    name: '',
    members: [],
    stp: false,
    mode: 'active_backup',
    parent: '',
    id: '10',
    ipv4: 'off',
    address: '',
    prefix: '24',
    gateway: '',
    ipv6: 'off',
  }
}

const offered = computed(() => {
  if (!props.network || kind.value === 'vlan') return []
  return candidates(props.network, props.devices, kind.value, props.editing?.name)
})
const parentChoices = computed(() => (props.network ? parents(props.network) : []))

// A new device starts with a free name; a change with what it has.
watch(open, (isOpen) => {
  if (!isOpen) return
  failure.value = null
  plan.value = null
  planProblem.value = null
  const next = blank()
  const editing = props.editing
  if (editing) {
    next.name = editing.name
    next.members = [...editing.members]
    next.stp = editing.stp ?? false
    next.mode = editing.mode ?? 'active_backup'
  } else if (props.network) {
    if (kind.value === 'vlan') {
      next.parent = parentChoices.value[0]?.name ?? ''
      next.name = vlanName(next.parent, Number(next.id))
    } else {
      next.name = freeName(kind.value === 'bond' ? 'bond' : 'br', props.network, props.devices)
    }
  }
  form.value = next
})

// A VLAN's name follows its parent and id until typed by hand.
const typedName = ref(false)
watch(open, () => (typedName.value = false))
watch(
  () => [form.value.parent, form.value.id],
  () => {
    if (kind.value === 'vlan' && !props.editing && !typedName.value) {
      form.value.name = vlanName(form.value.parent, Number(form.value.id))
    }
  },
)

function toggle(member: string, on: boolean): void {
  const members = form.value.members.filter((m) => m !== member)
  form.value.members = on ? [...members, member] : members
}

function vlanSettings(f: Form): InterfaceSettingsInput {
  const ipv4: InterfaceSettingsInput['ipv4'] =
    f.ipv4 === 'static'
      ? {
          mode: 'static',
          address: f.address.trim(),
          prefix: Number(f.prefix),
          gateway: f.gateway.trim() || null,
        }
      : { mode: f.ipv4 }
  return { ipv4, ipv6: f.ipv6, dns: [] }
}

function newDevice(f: Form): NewDeviceInput {
  if (kind.value === 'vlan') {
    return {
      kind: 'vlan',
      name: f.name.trim(),
      parent: f.parent,
      id: Number(f.id),
      settings: vlanSettings(f),
    }
  }
  if (kind.value === 'bond') {
    return { kind: 'bond', name: f.name.trim(), members: f.members, mode: f.mode }
  }
  return { kind: 'bridge', name: f.name.trim(), members: f.members, stp: f.stp }
}

function planInput(f: Form): DevicePlanInput {
  const editing = props.editing
  if (!editing) return { action: 'make', device: newDevice(f) }
  return editing.kind === 'bond'
    ? { action: 'change', name: editing.name, members: f.members, mode: f.mode, stp: null }
    : { action: 'change', name: editing.name, members: f.members, mode: null, stp: f.stp }
}

// What changes, from the server, a moment after each change of the form.
let asked = 0
let timer: ReturnType<typeof setTimeout> | undefined
onUnmounted(() => clearTimeout(timer))
watch(
  form,
  (f) => {
    if (!open.value) return
    clearTimeout(timer)
    const ask = ++asked
    planning.value = true
    timer = setTimeout(() => {
      actions
        .plan(planInput(f))
        .then((found) => {
          if (ask !== asked) return
          plan.value = found
          planProblem.value = null
        })
        .catch((problem: unknown) => {
          if (ask !== asked) return
          plan.value = null
          planProblem.value = problemMessage(problem)
        })
        .finally(() => {
          if (ask === asked) planning.value = false
        })
    }, 300)
  },
  { deep: true },
)

const title = computed(() =>
  props.editing
    ? t('network.device.titles.change', { name: props.editing.name })
    : t(`network.device.titles.${kind.value}`),
)
const hint = computed(() =>
  props.editing ? t('network.device.hints.change') : t(`network.device.hints.${kind.value}`),
)
/** Members join: firewall rules that name them see nothing of theirs now. */
const joining = computed(
  () => plan.value?.interfaces.some((fate) => fate.becomes.fate === 'member') ?? false,
)

async function submit(): Promise<void> {
  failure.value = null
  saving.value = true
  try {
    const f = form.value
    const editing = props.editing
    if (editing) {
      await actions.changeDevice(editing.name, {
        members: f.members,
        mode: editing.kind === 'bond' ? f.mode : null,
        stp: editing.kind === 'bridge' ? f.stp : null,
      })
    } else {
      await actions.makeDevice(newDevice(f))
    }
    open.value = false
    emit('done')
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}
</script>

<!-- Makes a bridge, bond or VLAN, or changes one's members; shows first what becomes of each interface. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="title"
    :description="hint"
    :close-label="t('desktop.close')"
    wide
  >
    <form class="grid gap-4 md:grid-cols-2" @submit.prevent="submit">
      <div class="grid content-start gap-3">
        <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
        <div v-if="!editing" class="grid gap-1.5">
          <Label for="device-name">{{ t('network.device.name') }}</Label>
          <Input
            id="device-name"
            v-model="form.name"
            :maxlength="NAME_MAX"
            required
            @input="typedName = true"
          />
          <p class="text-xs text-muted-foreground">{{ t('network.device.nameHint') }}</p>
        </div>

        <template v-if="kind === 'vlan'">
          <div class="grid grid-cols-[1fr_7rem] gap-2">
            <div class="grid gap-1.5">
              <Label for="device-parent">{{ t('network.device.parent') }}</Label>
              <NativeSelect id="device-parent" v-model="form.parent">
                <option v-for="item in parentChoices" :key="item.name" :value="item.name">
                  {{ item.name }}
                </option>
              </NativeSelect>
            </div>
            <div class="grid gap-1.5">
              <Label for="device-id">{{ t('network.device.id') }}</Label>
              <Input id="device-id" v-model="form.id" type="number" min="1" max="4094" required />
            </div>
          </div>
          <p class="text-xs text-muted-foreground">{{ t('network.device.idHint') }}</p>
          <div class="grid gap-1.5">
            <Label for="device-ipv4">IPv4</Label>
            <NativeSelect id="device-ipv4" v-model="form.ipv4">
              <option value="off">{{ t('network.change.ipv4Off') }}</option>
              <option value="dhcp">{{ t('network.change.dhcp') }}</option>
              <option value="static">{{ t('network.change.static') }}</option>
            </NativeSelect>
          </div>
          <template v-if="form.ipv4 === 'static'">
            <div class="grid grid-cols-[1fr_5rem] gap-2">
              <div class="grid gap-1.5">
                <Label for="device-address">{{ t('network.change.address') }}</Label>
                <Input
                  id="device-address"
                  v-model="form.address"
                  placeholder="10.10.0.2"
                  required
                />
              </div>
              <div class="grid gap-1.5">
                <Label for="device-prefix">{{ t('network.change.prefix') }}</Label>
                <Input id="device-prefix" v-model="form.prefix" type="number" min="1" max="32" />
              </div>
            </div>
            <div class="grid gap-1.5">
              <Label for="device-gateway">{{ t('network.gateway') }}</Label>
              <Input id="device-gateway" v-model="form.gateway" />
            </div>
          </template>
          <div class="grid gap-1.5">
            <Label for="device-ipv6">IPv6</Label>
            <NativeSelect id="device-ipv6" v-model="form.ipv6">
              <option value="off">{{ t('network.change.ipv6Off') }}</option>
              <option value="automatic">{{ t('network.change.ipv6Automatic') }}</option>
            </NativeSelect>
          </div>
        </template>

        <template v-else>
          <fieldset class="grid gap-1.5">
            <legend class="mb-1.5 text-sm font-medium">{{ t('network.device.members') }}</legend>
            <label
              v-for="item in offered"
              :key="item.name"
              class="flex items-center gap-2 rounded-md border px-3 py-2 text-sm"
            >
              <input
                type="checkbox"
                :checked="form.members.includes(item.name)"
                @change="toggle(item.name, ($event.target as HTMLInputElement).checked)"
              />
              <span class="font-mono">{{ item.name }}</span>
              <span class="truncate text-xs text-muted-foreground">
                {{ item.addresses.find((a) => a.scope === 'global')?.address ?? item.mac ?? '' }}
              </span>
              <span
                v-if="form.members.indexOf(item.name) >= 0"
                class="ml-auto text-xs text-muted-foreground tabular-nums"
              >
                {{ form.members.indexOf(item.name) + 1 }}
              </span>
            </label>
            <p v-if="!offered.length" class="text-sm text-muted-foreground">
              {{ t('network.device.noCandidates') }}
            </p>
            <p class="text-xs text-muted-foreground">
              {{
                kind === 'bond'
                  ? t('network.device.bondMembersHint')
                  : t('network.device.membersHint')
              }}
            </p>
            <p
              v-if="kind === 'bridge' && !form.members.length"
              class="text-xs text-muted-foreground"
            >
              {{ t('network.device.noMembersBridge') }}
            </p>
          </fieldset>
          <div v-if="kind === 'bond'" class="grid gap-1.5">
            <Label for="device-mode">{{ t('network.device.mode') }}</Label>
            <NativeSelect id="device-mode" v-model="form.mode">
              <option v-for="mode in MODES" :key="mode" :value="mode">
                {{ t(`network.devices.modes.${mode}`) }}
              </option>
            </NativeSelect>
            <p class="text-xs text-muted-foreground">
              {{ t(`network.devices.modeHints.${form.mode}`) }}
            </p>
          </div>
          <label v-else class="flex items-start gap-2 text-sm">
            <input v-model="form.stp" type="checkbox" class="mt-1" />
            <span>
              {{ t('network.device.stp') }}
              <span class="block text-xs text-muted-foreground">{{
                t('network.device.stpHint')
              }}</span>
            </span>
          </label>
        </template>
      </div>

      <section class="grid content-start gap-2 rounded-lg border bg-muted/30 p-3 text-sm">
        <h3 class="font-medium">{{ t('network.device.plan') }}</h3>
        <p v-if="planning" class="flex items-center gap-2 text-muted-foreground" role="status">
          <LoaderCircle class="size-4 animate-spin" />
          {{ t('network.device.planning') }}
        </p>
        <p v-else-if="planProblem" class="text-destructive" role="alert">{{ planProblem }}</p>
        <ul v-else-if="plan" class="grid gap-1.5">
          <li v-for="fate in plan.interfaces" :key="fate.interface" class="grid">
            <span class="font-mono">{{ fate.interface }}</span>
            <span class="text-muted-foreground">{{ describeFate(fate, t) }}</span>
          </li>
        </ul>
        <p v-if="joining" class="text-xs text-muted-foreground">
          {{ t('network.device.firewallHint') }}
        </p>
        <p class="text-xs text-muted-foreground">{{ t('network.device.hints.confirm') }}</p>
      </section>

      <div class="flex justify-end gap-2 md:col-span-2">
        <Button type="submit" :disabled="saving || planning || !plan">
          {{ editing ? t('network.device.apply') : t('network.device.make') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
