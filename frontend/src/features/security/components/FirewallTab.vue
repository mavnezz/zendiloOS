<script setup lang="ts">
import {
  ArrowDown,
  ArrowUp,
  GripVertical,
  Pencil,
  Plus,
  ShieldAlert,
  Trash2,
} from 'lucide-vue-next'
import { computed, onUnmounted, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { ApiProblem } from '@/shared/api/client'
import type { FirewallRule, FirewallService } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { PageDescription, PageFooter, SettingRow } from '@/shared/ui/page'
import { RadioGroup, RadioGroupItem } from '@/shared/ui/radio-group'
import { Switch } from '@/shared/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useFirewall, useFirewallActions } from '../firewall'
import {
  type FirewallDraft,
  fromDraft,
  moved,
  plainRule,
  portsOf,
  protocolOf,
  ruleId,
  sameDraft,
  toDraft,
} from '../lib/firewall'
import FirewallAllowDialog from './FirewallAllowDialog.vue'
import FirewallRuleDialog from './FirewallRuleDialog.vue'

const { t } = useI18n()
const { data: setup, error: loadError } = useFirewall()
const actions = useFirewallActions()

/** What is in force: a change waiting for confirmation, or the settings kept. */
const inForce = computed(() => setup.value?.pending?.settings ?? setup.value?.current)
const pending = computed(() => Boolean(setup.value?.pending))
const services = computed(() => setup.value?.services ?? [])

const draft = ref<FirewallDraft>()
const base = shallowRef<FirewallDraft>()
const saving = ref(false)
const failure = ref<string | null>(null)
const dirty = computed(() =>
  Boolean(draft.value && base.value && !sameDraft(draft.value, base.value)),
)

function reset(): void {
  if (!inForce.value) return
  base.value = toDraft(inForce.value)
  draft.value = toDraft(inForce.value)
  failure.value = null
}
watch(
  inForce,
  () => {
    if (!base.value || !dirty.value) reset()
  },
  { immediate: true },
)

// The rule being edited: its place, or -1 for a new one.
const editing = ref(-1)
const editingOpen = ref(false)
const edited = computed(() =>
  editing.value >= 0 ? (draft.value?.rules[editing.value] ?? null) : null,
)

function addRule(): void {
  editing.value = -1
  editingOpen.value = true
}

function editRule(index: number): void {
  editing.value = index
  editingOpen.value = true
}

function saveRule(rule: FirewallRule): void {
  if (!draft.value) return
  const rules = [...draft.value.rules]
  if (editing.value >= 0) rules[editing.value] = { ...rule, id: rules[editing.value]!.id }
  else rules.push({ ...rule, id: ruleId() })
  draft.value.rules = rules
}

function removeRule(index: number): void {
  if (draft.value) draft.value.rules = draft.value.rules.filter((_, i) => i !== index)
}

function move(from: number, to: number): void {
  if (draft.value) draft.value.rules = moved(draft.value.rules, from, to)
}

// Dragging rules into order.
const dragging = ref<number | null>(null)
function dragStart(index: number, event: DragEvent): void {
  dragging.value = index
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', String(index))
  }
}
function drop(index: number): void {
  if (dragging.value !== null) move(dragging.value, index)
  dragging.value = null
}

// Applying, with the browser's own way in checked.
const lockOut = ref(false)
async function apply(lockOutMeant = false): Promise<void> {
  const value = draft.value
  if (!value) return
  saving.value = true
  failure.value = null
  try {
    await actions.apply({ ...fromDraft(value), lock_out: lockOutMeant })
    // Applied: nothing left to apply; gone back unconfirmed, the page shows the rules from before.
    base.value = toDraft(fromDraft(value))
  } catch (error) {
    if (error instanceof ApiProblem && error.code === 'would_lock_out') lockOut.value = true
    else failure.value = problemMessage(error)
  } finally {
    saving.value = false
  }
}

async function confirm(): Promise<void> {
  failure.value = null
  try {
    await actions.confirm()
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

// Services nobody reaches, let through from the top of the list.
const blocked = computed(() =>
  (setup.value?.blocked ?? [])
    .map((key) => services.value.find((s) => s.key === key))
    .filter((s): s is FirewallService => Boolean(s)),
)
const allowing = ref<FirewallService | null>(null)
const allowOpen = ref(false)
const allowBusy = ref(false)
function openAllow(service: FirewallService): void {
  allowing.value = service
  allowOpen.value = true
}
async function allow(sources: string[]): Promise<void> {
  const service = allowing.value
  const settings = inForce.value
  if (!service || !settings) return
  const rules = sources.map((source) =>
    plainRule({
      enabled: true,
      interface: null,
      service: service.service,
      project: service.project ?? null,
      source,
      action: 'allow',
    }),
  )
  allowBusy.value = true
  failure.value = null
  try {
    await actions.apply({ ...settings, rules: [...rules, ...settings.rules] })
  } catch (error) {
    failure.value = problemMessage(error)
  } finally {
    allowBusy.value = false
    allowOpen.value = false
  }
}

function serviceName(service: {
  service: FirewallRule['service']
  project?: string | null
}): string {
  return service.service === 'project'
    ? t('firewall.project', { name: service.project ?? '' })
    : t(`firewall.services.${service.service}`)
}
function protocolLabel(rule: FirewallRule): string {
  const protocol = protocolOf(rule, services.value)
  return protocol ? t(`firewall.protocols.${protocol}`) : '–'
}
function servicePorts(service: FirewallService): string {
  return [service.tcp, service.udp].filter(Boolean).join(' / ')
}
</script>

<!--
  The firewall, as on the appliances people know: on or off, the rules in
  their order (dragged or moved with the arrows), what happens when none
  matches, the services nobody reaches with a way to let them through. A
  change applies at once and goes back by itself unless kept in time.
-->
<template>
  <div class="grid max-w-5xl gap-6">
    <Alert v-if="loadError" variant="destructive">{{ problemMessage(loadError) }}</Alert>
    <div
      v-if="setup?.pending"
      class="grid gap-2 rounded-xl border border-amber-500/50 bg-amber-500/10 p-4 text-sm"
      role="status"
    >
      <p class="font-medium">{{ t('firewall.pending.title') }}</p>
      <p>{{ t('firewall.pending.hint', { seconds: secondsLeft }) }}</p>
      <div class="flex justify-end">
        <Button size="sm" @click="confirm">{{ t('firewall.pending.confirm') }}</Button>
      </div>
    </div>
    <section
      v-if="blocked.length"
      class="grid gap-2 rounded-xl border border-destructive/40 bg-destructive/5 p-4 text-sm"
      :aria-label="t('firewall.blocked.title')"
    >
      <p class="flex items-center gap-2 font-medium">
        <ShieldAlert class="size-4 text-destructive" />
        {{ t('firewall.blocked.title') }}
      </p>
      <p class="text-muted-foreground">{{ t('firewall.blocked.hint') }}</p>
      <ul class="grid gap-1.5">
        <li v-for="service in blocked" :key="service.key" class="flex items-center gap-3">
          <span class="flex-1">
            {{ serviceName(service) }}
            <span class="ml-1 font-mono text-xs text-muted-foreground">{{
              servicePorts(service)
            }}</span>
          </span>
          <Button
            size="sm"
            variant="secondary"
            :disabled="dirty || pending"
            @click="openAllow(service)"
          >
            {{ t('firewall.blocked.allow') }}
          </Button>
        </li>
      </ul>
    </section>

    <div v-if="!draft" class="h-48 animate-pulse rounded-lg bg-muted" />
    <template v-else>
      <SettingRow :label="t('firewall.title')" for="firewall-enabled">
        <label class="flex items-center gap-3 pt-1 text-sm">
          <Switch id="firewall-enabled" v-model="draft.enabled" :disabled="pending" />
          {{ t('firewall.enable') }}
        </label>
        <p class="text-xs text-muted-foreground">{{ t('firewall.description') }}</p>
      </SettingRow>

      <SettingRow :label="t('firewall.rules')">
        <div class="flex flex-wrap items-center gap-3">
          <Button class="w-fit" :disabled="pending" @click="addRule">
            <Plus />
            {{ t('firewall.add') }}
          </Button>
          <p class="text-xs text-muted-foreground">{{ t('firewall.rulesHint') }}</p>
        </div>
        <div class="overflow-x-auto rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead class="w-8"
                  ><span class="sr-only">{{ t('firewall.drag') }}</span></TableHead
                >
                <TableHead>{{ t('firewall.columns.enabled') }}</TableHead>
                <TableHead>{{ t('firewall.columns.interface') }}</TableHead>
                <TableHead>{{ t('firewall.columns.service') }}</TableHead>
                <TableHead>{{ t('firewall.columns.protocol') }}</TableHead>
                <TableHead>{{ t('firewall.columns.source') }}</TableHead>
                <TableHead>{{ t('firewall.columns.action') }}</TableHead>
                <TableHead class="w-0"
                  ><span class="sr-only">{{ t('firewall.columns.manage') }}</span></TableHead
                >
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow
                v-for="(rule, index) in draft.rules"
                :key="rule.id"
                :draggable="!pending"
                :class="dragging === index && 'opacity-50'"
                data-rule
                @dragstart="dragStart(index, $event)"
                @dragover.prevent
                @drop.prevent="drop(index)"
                @dragend="dragging = null"
              >
                <TableCell class="cursor-grab text-muted-foreground" :title="t('firewall.drag')">
                  <GripVertical class="size-4" />
                </TableCell>
                <TableCell>
                  <Switch
                    v-model="rule.enabled"
                    :disabled="pending"
                    :aria-label="t('firewall.columns.enabled')"
                  />
                </TableCell>
                <TableCell>{{ rule.interface || t('firewall.anyInterface') }}</TableCell>
                <TableCell>
                  <div>{{ serviceName(rule) }}</div>
                  <div class="font-mono text-xs text-muted-foreground">
                    {{ portsOf(rule, services) }}
                  </div>
                </TableCell>
                <TableCell>{{ protocolLabel(rule) }}</TableCell>
                <TableCell class="font-mono text-[13px]">
                  {{ rule.source === 'any' ? t('firewall.anySource') : rule.source }}
                </TableCell>
                <TableCell>
                  <Badge :variant="rule.action === 'allow' ? 'success' : 'destructive'">
                    {{ t(`firewall.actions.${rule.action}`) }}
                  </Badge>
                </TableCell>
                <TableCell>
                  <div class="flex justify-end gap-0.5">
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      :aria-label="t('firewall.moveUp')"
                      :disabled="pending || index === 0"
                      @click="move(index, index - 1)"
                    >
                      <ArrowUp />
                    </Button>
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      :aria-label="t('firewall.moveDown')"
                      :disabled="pending || index === draft.rules.length - 1"
                      @click="move(index, index + 1)"
                    >
                      <ArrowDown />
                    </Button>
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      :aria-label="t('firewall.edit')"
                      :disabled="pending"
                      @click="editRule(index)"
                    >
                      <Pencil />
                    </Button>
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      :aria-label="t('firewall.remove')"
                      :disabled="pending"
                      @click="removeRule(index)"
                    >
                      <Trash2 />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
              <TableRow v-if="!draft.rules.length">
                <TableCell colspan="8" class="py-8 text-center text-muted-foreground">
                  {{ t('firewall.noRules') }}
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </SettingRow>

      <SettingRow :label="t('firewall.fallback')">
        <RadioGroup
          v-model="draft.fallback"
          :label="t('firewall.fallback')"
          :disabled="pending"
          class="pt-1.5"
        >
          <RadioGroupItem value="allow">{{ t('firewall.allowAll') }}</RadioGroupItem>
          <RadioGroupItem value="deny">{{ t('firewall.denyAll') }}</RadioGroupItem>
        </RadioGroup>
        <p class="text-xs text-muted-foreground">{{ t('firewall.fallbackHint') }}</p>
      </SettingRow>
    </template>
  </div>
  <PageDescription>
    <p>{{ t('firewall.help') }}</p>
  </PageDescription>
  <div class="grow" aria-hidden="true" />
  <PageFooter
    v-if="draft"
    :dirty="dirty"
    :saving="saving"
    :error="failure"
    @reset="reset()"
    @apply="apply()"
  />
  <FirewallRuleDialog
    v-model:open="editingOpen"
    :rule="edited"
    :services="services"
    :interfaces="setup?.interfaces ?? []"
    :local-networks="setup?.local_networks ?? []"
    @save="saveRule"
  />
  <FirewallAllowDialog
    v-model:open="allowOpen"
    :name="allowing ? serviceName(allowing) : ''"
    :local-networks="setup?.local_networks ?? []"
    :busy="allowBusy"
    @allow="allow"
  />
  <ConfirmDialog
    v-model:open="lockOut"
    :title="t('firewall.lockOut.title')"
    :description="t('firewall.lockOut.description', { seconds: setup?.confirm_within ?? 90 })"
    :confirm-label="t('firewall.lockOut.confirm')"
    :cancel-label="t('desktop.cancel')"
    destructive
    @confirm="apply(true)"
  />
</template>
