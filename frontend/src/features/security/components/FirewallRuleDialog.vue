<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type {
  FirewallAction,
  FirewallProtocol,
  FirewallRule,
  FirewallService,
  FirewallServiceKind,
} from '@/shared/api/types'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { RadioGroup, RadioGroupItem } from '@/shared/ui/radio-group'
import { Switch } from '@/shared/ui/switch'

import { plainRule, portsValid, sourceValid } from '../lib/firewall'

/** The rule to change; none: a new one. */
const props = defineProps<{
  rule: FirewallRule | null
  services: FirewallService[]
  interfaces: string[]
  localNetworks: string[]
}>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ save: [rule: FirewallRule] }>()

const { t } = useI18n()

const KINDS: FirewallServiceKind[] = [
  'all',
  'web_interface',
  'smb',
  'nfs',
  'ssh',
  'discovery',
  'ping',
  'project',
  'ports',
]

const enabled = ref(true)
const iface = ref('')
const service = ref<FirewallServiceKind>('all')
const project = ref('')
const protocol = ref<FirewallProtocol>('tcp')
const ports = ref('')
/** `any`, a local network, or `custom` with `source` typed in. */
const sourceMode = ref('any')
const source = ref('')
const action = ref<FirewallAction>('allow')

/** Projects that publish ports, and the rule's own even if it does not now. */
const projects = computed(() => {
  const names = props.services.filter((s) => s.service === 'project').map((s) => s.project ?? '')
  const own = props.rule?.project
  return own && !names.includes(own) ? [own, ...names] : names
})
const interfaces = computed(() => {
  const own = props.rule?.interface
  return own && !props.interfaces.includes(own) ? [own, ...props.interfaces] : props.interfaces
})

watch(open, (isOpen) => {
  if (!isOpen) return
  const rule = props.rule
  enabled.value = rule?.enabled ?? true
  iface.value = rule?.interface ?? ''
  service.value = rule?.service ?? 'all'
  project.value = rule?.project ?? projects.value[0] ?? ''
  protocol.value = rule?.protocol ?? 'tcp'
  ports.value = rule?.ports ?? ''
  const given = rule?.source ?? 'any'
  sourceMode.value =
    given === 'any' || props.localNetworks.includes(given) ? given : rule ? 'custom' : 'any'
  source.value = sourceMode.value === 'custom' ? given : ''
  action.value = rule?.action ?? 'allow'
})

function portsLabel(kind: FirewallServiceKind): string {
  const known = props.services.find((s) => s.service === kind && kind !== 'project')
  const list = known ? [known.tcp, known.udp].filter(Boolean).join(' / ') : ''
  return list ? `${t(`firewall.services.${kind}`)} (${list})` : t(`firewall.services.${kind}`)
}

const chosenSource = computed(() =>
  sourceMode.value === 'custom' ? source.value.trim() : sourceMode.value,
)
const problem = computed(() => {
  if (service.value === 'project' && !project.value) return t('firewall.rule.noProjects')
  if (service.value === 'ports' && !portsValid(ports.value)) return t('firewall.rule.invalidPorts')
  if (sourceMode.value === 'custom' && (!source.value.trim() || !sourceValid(source.value)))
    return t('firewall.rule.invalidSource')
  return null
})

function save(): void {
  if (problem.value) return
  emit(
    'save',
    plainRule({
      enabled: enabled.value,
      interface: iface.value || null,
      service: service.value,
      project: project.value,
      protocol: protocol.value,
      ports: ports.value,
      source: chosenSource.value,
      action: action.value,
    }),
  )
  open.value = false
}
</script>

<!--
  One rule: on which network connection, for which service or ports, from
  whom, and whether it lets them in.
-->
<template>
  <ModalDialog
    v-model:open="open"
    :title="rule ? t('firewall.rule.edit') : t('firewall.rule.add')"
    :close-label="t('desktop.close')"
  >
    <form id="firewall-rule" class="grid gap-4" @submit.prevent="save">
      <label class="flex items-center gap-3 text-sm">
        <Switch id="firewall-rule-enabled" v-model="enabled" />
        {{ t('firewall.rule.enabled') }}
      </label>
      <div class="grid gap-1.5">
        <Label for="firewall-rule-interface">{{ t('firewall.rule.interface') }}</Label>
        <NativeSelect id="firewall-rule-interface" v-model="iface">
          <option value="">{{ t('firewall.anyInterface') }}</option>
          <option v-for="name in interfaces" :key="name" :value="name">{{ name }}</option>
        </NativeSelect>
      </div>
      <div class="grid gap-1.5">
        <Label for="firewall-rule-service">{{ t('firewall.rule.service') }}</Label>
        <NativeSelect id="firewall-rule-service" v-model="service">
          <option v-for="kind in KINDS" :key="kind" :value="kind">{{ portsLabel(kind) }}</option>
        </NativeSelect>
      </div>
      <div v-if="service === 'project'" class="grid gap-1.5">
        <Label for="firewall-rule-project">{{ t('firewall.rule.project') }}</Label>
        <NativeSelect id="firewall-rule-project" v-model="project" :disabled="!projects.length">
          <option v-for="name in projects" :key="name" :value="name">{{ name }}</option>
        </NativeSelect>
        <p class="text-xs text-muted-foreground">
          {{ projects.length ? t('firewall.rule.projectHint') : t('firewall.rule.noProjects') }}
        </p>
      </div>
      <div v-if="service === 'ports'" class="grid gap-1.5">
        <Label for="firewall-rule-ports">{{ t('firewall.rule.ports') }}</Label>
        <div class="flex gap-2">
          <NativeSelect v-model="protocol" class="w-36" :aria-label="t('firewall.rule.protocol')">
            <option value="tcp">{{ t('firewall.protocols.tcp') }}</option>
            <option value="udp">{{ t('firewall.protocols.udp') }}</option>
            <option value="both">{{ t('firewall.protocols.both') }}</option>
          </NativeSelect>
          <Input
            id="firewall-rule-ports"
            v-model="ports"
            class="font-mono text-dense"
            placeholder="80, 443, 8000-8100"
            :aria-invalid="!!ports && !portsValid(ports)"
          />
        </div>
        <p class="text-xs text-muted-foreground">{{ t('firewall.rule.portsHint') }}</p>
      </div>
      <div class="grid gap-1.5">
        <Label for="firewall-rule-source">{{ t('firewall.rule.source') }}</Label>
        <NativeSelect id="firewall-rule-source" v-model="sourceMode">
          <option value="any">{{ t('firewall.anySource') }}</option>
          <option v-for="network in localNetworks" :key="network" :value="network">
            {{ t('firewall.rule.sourceLocal', { network }) }}
          </option>
          <option value="custom">{{ t('firewall.rule.sourceCustom') }}</option>
        </NativeSelect>
        <template v-if="sourceMode === 'custom'">
          <Input
            id="firewall-rule-custom"
            v-model="source"
            class="font-mono text-dense"
            placeholder="192.168.1.0/24"
            :aria-label="t('firewall.rule.sourceCustom')"
            :aria-invalid="!!source && !sourceValid(source)"
          />
          <p class="text-xs text-muted-foreground">{{ t('firewall.rule.sourceHint') }}</p>
        </template>
      </div>
      <RadioGroup v-model="action" :label="t('firewall.rule.action')">
        <RadioGroupItem value="allow">{{ t('firewall.actions.allow') }}</RadioGroupItem>
        <RadioGroupItem value="deny">{{ t('firewall.actions.deny') }}</RadioGroupItem>
      </RadioGroup>
      <p v-if="problem" class="text-sm text-destructive" role="alert">{{ problem }}</p>
    </form>
    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button type="submit" form="firewall-rule" :disabled="!!problem">{{
        t('firewall.rule.done')
      }}</Button>
    </template>
  </ModalDialog>
</template>
