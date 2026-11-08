<script setup lang="ts">
import {
  ChevronDown,
  ChevronRight,
  Copy,
  LoaderCircle,
  Network,
  Pencil,
  Plus,
  Trash2,
  TriangleAlert,
} from 'lucide-vue-next'
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NfsRule, Share, ShareCompressionKind, ShareRule } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { PageTabs } from '@/shared/ui/page'

import { useEntryFacts } from '../../api'
import { effectiveLevel, type Level } from '../../lib/permissions'
import { useShareActions, useSubjects } from '../../sharing'
import NfsRuleDialog from './NfsRuleDialog.vue'

export type PropertiesTab = 'general' | 'permissions' | 'sharing' | 'nfs'

/** A folder directly on a volume: what it holds, who may use it, whether and how it is on the network. */
const props = defineProps<{ share: Share | null; tab?: PropertiesTab }>()
const open = defineModel<boolean>('open', { default: false })

const { t, locale } = useI18n()
/** A count as the interface's language writes it. */
const count = (value: number) => value.toLocaleString(locale.value)
const actions = useShareActions()
const notifications = useNotifications()
const { accounts, groups } = useSubjects()

const active = ref<PropertiesTab>('general')
const description = ref('')
const shared = ref(true)
const recycleBin = ref(true)
/** In GiB as typed; empty means no quota. */
const quota = ref('')
const compression = ref<ShareCompressionKind>('lz4')
const recordSize = ref(131072)
const GIB = 1024 ** 3
const RECORD_SIZES = [16384, 131072, 1048576]
/** `account:3` or `group:5` → level. */
const levels = ref<Record<string, Level>>({})
const nfs = ref<NfsRule[]>([])
/** The NFS rule being edited by its place in the list, `-1` a new one. */
const editing = ref<number | null>(null)
/** Groups opened to show their members. */
const expanded = reactive(new Set<number>())
const error = ref<string | null>(null)
const pending = ref(false)

const key = (kind: string, id: number) => `${kind}:${id}`
const tabs = (['general', 'permissions', 'sharing', 'nfs'] as const).map((id) => ({
  id,
  label: t(`explorer.properties.tabs.${id}`),
}))

/** Counted on the NAS while the dialog is open; closing it calls the count off. */
const { facts, counting } = useEntryFacts(() =>
  open.value && props.share ? { place: props.share.id, path: '', snapshot: null } : null,
)

/** How computers reach it: the name this page was opened with. */
const host = typeof window === 'undefined' ? 'nas' : window.location.hostname
const smbPath = computed(() => (props.share ? `\\\\${host}\\${props.share.name}` : ''))
const nfsPath = computed(() => (props.share ? `${host}:${props.share.path}` : ''))

watch(open, (isOpen) => {
  const share = props.share
  if (!isOpen || !share) return
  active.value = props.tab ?? 'general'
  description.value = share.description ?? ''
  shared.value = share.shared
  recycleBin.value = share.recycle_bin
  quota.value = share.settings.quota_bytes
    ? String(Math.round((share.settings.quota_bytes / GIB) * 10) / 10)
    : ''
  compression.value = share.settings.compression
  recordSize.value = share.settings.record_size_bytes
  levels.value = Object.fromEntries(
    share.access.map((rule) => [key(rule.kind, rule.id), rule.access]),
  )
  nfs.value = share.nfs.map((rule) => ({
    network: rule.network,
    write: rule.write,
    squash: rule.squash ?? 'root',
    async: rule.async ?? false,
    insecure: rule.insecure ?? false,
  }))
  expanded.clear()
  editing.value = null
  error.value = null
})

/** A level in one column of a row: chosen again, the row has no rule. */
function choose(subject: string, level: Exclude<Level, 'none'>): void {
  const next = { ...levels.value }
  if (next[subject] === level) delete next[subject]
  else next[subject] = level
  levels.value = next
}

function groupsOf(account: number): number[] {
  return (groups.data.value ?? []).filter((g) => g.members.includes(account)).map((g) => g.id)
}

/** What holds for an account with the rules as they are now chosen. */
function effective(account: number) {
  return effectiveLevel(
    levels.value[key('account', account)],
    groupsOf(account).map((group) => levels.value[key('group', group)]),
  )
}

function accountName(id: number): string {
  const account = accounts.data.value?.find((a) => a.id === id)
  return account ? (account.full_name ?? account.name) : `#${id}`
}

function toggleGroup(id: number): void {
  if (expanded.has(id)) expanded.delete(id)
  else expanded.add(id)
}

const COLUMNS = ['deny', 'read_write', 'read'] as const

function rules(): ShareRule[] {
  return Object.entries(levels.value)
    .filter(([, level]) => level === 'deny' || level === 'read' || level === 'read_write')
    .map(([subject, level]) => {
      const [kind, id] = subject.split(':')
      return {
        kind: kind as ShareRule['kind'],
        id: Number(id),
        access: level as ShareRule['access'],
      }
    })
}

function settings() {
  const gib = Number(quota.value.replace(',', '.'))
  return {
    quota_bytes: quota.value.trim() === '' || !Number.isFinite(gib) ? null : Math.round(gib * GIB),
    compression: compression.value,
    record_size_bytes: recordSize.value,
  }
}

function saveRule(rule: NfsRule): void {
  const at = editing.value
  if (at === null) return
  if (at < 0) nfs.value = [...nfs.value, rule]
  else nfs.value = nfs.value.map((old, index) => (index === at ? rule : old))
}

const editedRule = computed(() =>
  editing.value === null || editing.value < 0 ? null : (nfs.value[editing.value] ?? null),
)

async function copy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
    notifications.notify('success', t('fileServices.copied', { text }))
  } catch {
    notifications.notify('error', t('fileServices.copyFailed'))
  }
}

async function submit(): Promise<void> {
  const share = props.share
  if (!share) return
  pending.value = true
  error.value = null
  try {
    const { job } = await actions.update(share.id, {
      description: description.value,
      shared: shared.value,
      recycle_bin: recycleBin.value,
      access: rules(),
      nfs: nfs.value,
      settings: settings(),
    })
    notifications.notify(
      'success',
      t('shares.saved', { name: share.name }),
      job ? t('shares.savedHint') : undefined,
    )
    open.value = false
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}

const levelText = (level: ReturnType<typeof effective>) =>
  level ? t(`shares.levels.${level}`) : t('explorer.properties.noAccess')
const levelClass = (level: ReturnType<typeof effective>) =>
  level === 'deny' ? 'text-destructive' : level ? '' : 'text-muted-foreground'
const select = 'h-9 rounded-md border bg-background px-3 text-sm shadow-xs'
const cell = 'px-2 py-1.5'
</script>

<template>
  <ModalDialog
    v-model:open="open"
    wide
    :title="t('explorer.properties.title', { name: share?.name })"
    :close-label="t('desktop.close')"
  >
    <form v-if="share" class="grid gap-4" @submit.prevent="submit">
      <PageTabs v-model="active" :tabs="tabs" :label="t('explorer.properties.tabsLabel')" />
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>

      <template v-if="active === 'general'">
        <dl class="grid grid-cols-[auto_1fr] items-baseline gap-x-6 gap-y-2 text-sm">
          <dt class="text-muted-foreground">{{ t('explorer.properties.name') }}</dt>
          <dd class="font-medium">{{ share.name }}</dd>
          <dt class="text-muted-foreground">{{ t('explorer.properties.location') }}</dt>
          <dd class="flex min-w-0 items-center gap-1">
            <span class="font-mono break-all">{{ share.path }}</span>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              class="size-7"
              :aria-label="t('explorer.properties.copy', { text: share.path })"
              @click="copy(share.path)"
            >
              <Copy class="size-3.5" />
            </Button>
          </dd>
          <template v-if="share.shared">
            <dt class="text-muted-foreground">SMB</dt>
            <dd class="flex min-w-0 items-center gap-1">
              <span class="font-mono break-all">{{ smbPath }}</span>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                class="size-7"
                :aria-label="t('explorer.properties.copy', { text: smbPath })"
                @click="copy(smbPath)"
              >
                <Copy class="size-3.5" />
              </Button>
            </dd>
            <template v-if="share.nfs.length">
              <dt class="text-muted-foreground">NFS</dt>
              <dd class="flex min-w-0 items-center gap-1">
                <span class="font-mono break-all">{{ nfsPath }}</span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  class="size-7"
                  :aria-label="t('explorer.properties.copy', { text: nfsPath })"
                  @click="copy(nfsPath)"
                >
                  <Copy class="size-3.5" />
                </Button>
              </dd>
            </template>
          </template>
          <template v-if="share.used_bytes != null">
            <dt class="text-muted-foreground">{{ t('explorer.properties.used') }}</dt>
            <dd>
              {{
                share.settings.quota_bytes
                  ? t('shares.storage.usedOfQuota', {
                      used: formatBytes(share.used_bytes),
                      quota: formatBytes(share.settings.quota_bytes),
                    })
                  : t('shares.storage.used', { used: formatBytes(share.used_bytes) })
              }}
            </dd>
          </template>
          <dt class="text-muted-foreground">{{ t('explorer.properties.contains') }}</dt>
          <dd role="status">
            <span v-if="counting" class="flex items-center gap-2 text-muted-foreground">
              <LoaderCircle class="size-4 animate-spin" />
              {{ t('explorer.properties.counting') }}
            </span>
            <template v-else-if="facts">
              {{
                t('explorer.properties.counted', {
                  files: count(facts.files),
                  folders: count(facts.folders),
                  hidden: count(facts.hidden),
                  bytes: formatBytes(facts.bytes),
                })
              }}
              <span v-if="facts.stopped" class="block text-xs text-muted-foreground">
                {{ t(`explorer.properties.stopped.${facts.stopped}`) }}
              </span>
            </template>
            <span v-else class="text-muted-foreground">{{
              t('explorer.properties.notCounted')
            }}</span>
          </dd>
          <template v-if="facts?.modified">
            <dt class="text-muted-foreground">{{ t('files.modified') }}</dt>
            <dd>{{ formatDateTime(facts.modified * 1000) }}</dd>
          </template>
          <dt class="text-muted-foreground">{{ t('explorer.properties.created') }}</dt>
          <dd>{{ formatDateTime((facts?.created ?? share.created_at) * 1000) }}</dd>
          <template v-if="facts?.owner">
            <dt class="text-muted-foreground">{{ t('explorer.properties.owner') }}</dt>
            <dd>{{ facts.owner }}</dd>
          </template>
        </dl>
        <div class="grid gap-2">
          <Label for="share-description">{{ t('shares.description') }}</Label>
          <Input id="share-description" v-model="description" autocomplete="off" maxlength="200" />
        </div>
        <label class="flex items-start gap-2 text-sm">
          <input v-model="recycleBin" type="checkbox" class="mt-0.5 size-4 accent-primary" />
          <span>
            {{ t('shares.recycleBin') }}
            <span class="block text-xs text-muted-foreground">{{
              t('shares.recycleBinHint')
            }}</span>
          </span>
        </label>
        <fieldset class="grid gap-3">
          <legend class="mb-1 text-sm font-medium">{{ t('shares.storage.title') }}</legend>
          <div class="grid gap-1">
            <Label for="share-quota">{{ t('shares.storage.quota') }}</Label>
            <Input
              id="share-quota"
              v-model="quota"
              inputmode="decimal"
              :placeholder="t('shares.storage.noQuota')"
            />
            <p class="text-xs text-muted-foreground">{{ t('shares.storage.quotaHint') }}</p>
          </div>
          <div class="grid grid-cols-2 gap-3">
            <div class="grid gap-1">
              <Label for="share-compression">{{ t('shares.storage.compression') }}</Label>
              <select id="share-compression" v-model="compression" :class="select">
                <option v-for="kind in ['lz4', 'zstd', 'off'] as const" :key="kind" :value="kind">
                  {{ t(`shares.storage.compressions.${kind}`) }}
                </option>
              </select>
            </div>
            <div class="grid gap-1">
              <Label for="share-record-size">{{ t('shares.storage.recordSize') }}</Label>
              <select id="share-record-size" v-model.number="recordSize" :class="select">
                <option v-for="size in RECORD_SIZES" :key="size" :value="size">
                  {{ t(`shares.storage.recordSizes.${size}`) }}
                </option>
              </select>
            </div>
          </div>
        </fieldset>
      </template>

      <div v-else-if="active === 'permissions'" class="grid gap-2">
        <p class="text-xs text-muted-foreground">{{ t('explorer.properties.accessHint') }}</p>
        <div class="max-h-80 overflow-auto rounded-lg border">
          <table class="w-full text-sm">
            <thead class="sticky top-0 bg-muted text-left text-xs text-muted-foreground">
              <tr>
                <th :class="cell">{{ t('explorer.properties.subject') }}</th>
                <th v-for="column in COLUMNS" :key="column" :class="cell" class="text-center">
                  {{ t(`shares.levels.${column}`) }}
                </th>
                <th :class="cell">{{ t('explorer.properties.effective') }}</th>
              </tr>
            </thead>
            <tbody>
              <template v-for="group in groups.data.value" :key="key('group', group.id)">
                <tr class="border-t">
                  <td :class="cell">
                    <button
                      type="button"
                      class="flex items-center gap-1 text-left"
                      :aria-expanded="expanded.has(group.id)"
                      :aria-label="t('explorer.properties.showMembers', { name: group.name })"
                      :disabled="!group.members.length"
                      @click="toggleGroup(group.id)"
                    >
                      <ChevronDown v-if="expanded.has(group.id)" class="size-3.5" />
                      <ChevronRight v-else class="size-3.5" />
                      <span class="font-medium">{{ group.name }}</span>
                      <span class="text-xs text-muted-foreground">({{ t('shares.group') }})</span>
                    </button>
                  </td>
                  <td v-for="column in COLUMNS" :key="column" :class="cell" class="text-center">
                    <input
                      type="checkbox"
                      class="size-4 accent-primary"
                      :checked="levels[key('group', group.id)] === column"
                      :aria-label="`${group.name}: ${t(`shares.levels.${column}`)}`"
                      @change="choose(key('group', group.id), column)"
                    />
                  </td>
                  <td :class="cell" class="text-muted-foreground">
                    {{ t('explorer.properties.members', group.members.length) }}
                  </td>
                </tr>
                <template v-if="expanded.has(group.id)">
                  <tr
                    v-for="member in group.members"
                    :key="`member-${group.id}-${member}`"
                    class="bg-muted/30"
                  >
                    <td :class="cell" class="pl-8">{{ accountName(member) }}</td>
                    <td :colspan="COLUMNS.length" />
                    <td :class="[cell, levelClass(effective(member))]">
                      {{ levelText(effective(member)) }}
                    </td>
                  </tr>
                </template>
              </template>
              <tr
                v-for="account in accounts.data.value"
                :key="key('account', account.id)"
                class="border-t"
              >
                <td :class="cell">{{ account.full_name ?? account.name }}</td>
                <td v-for="column in COLUMNS" :key="column" :class="cell" class="text-center">
                  <input
                    type="checkbox"
                    class="size-4 accent-primary"
                    :checked="levels[key('account', account.id)] === column"
                    :aria-label="`${account.name}: ${t(`shares.levels.${column}`)}`"
                    @change="choose(key('account', account.id), column)"
                  />
                </td>
                <td :class="[cell, levelClass(effective(account.id))]">
                  {{ levelText(effective(account.id)) }}
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <p class="text-xs text-muted-foreground">{{ t('explorer.properties.appliesToAll') }}</p>
      </div>

      <template v-else-if="active === 'sharing'">
        <label class="flex items-start gap-2 text-sm">
          <input v-model="shared" type="checkbox" class="mt-0.5 size-4 accent-primary" />
          <span>
            <span class="flex items-center gap-1.5">
              <Network class="size-4" />
              {{ t('explorer.properties.shared') }}
            </span>
            <span class="block text-xs text-muted-foreground">{{
              t('explorer.properties.sharedHint', { name: share.name })
            }}</span>
          </span>
        </label>
        <p v-if="shared" class="text-sm">
          {{ t('explorer.properties.reachedAs') }}
          <span class="font-mono">{{ smbPath }}</span>
        </p>
        <p class="text-xs text-muted-foreground">{{ t('explorer.properties.servicesHint') }}</p>
      </template>

      <div v-else class="grid gap-3">
        <Alert>
          <TriangleAlert class="text-amber-600 dark:text-amber-400" />
          {{ t('shares.nfsWarning') }}
        </Alert>
        <p v-if="!shared" class="text-sm text-muted-foreground">
          {{ t('explorer.nfs.notShared') }}
        </p>
        <div class="overflow-auto rounded-lg border">
          <table class="w-full text-sm">
            <thead class="bg-muted text-left text-xs text-muted-foreground">
              <tr>
                <th :class="cell">{{ t('explorer.nfs.client') }}</th>
                <th :class="cell">{{ t('explorer.nfs.permission') }}</th>
                <th :class="cell">{{ t('explorer.nfs.squash') }}</th>
                <th :class="cell">{{ t('explorer.nfs.asyncShort') }}</th>
                <th :class="cell">{{ t('explorer.nfs.insecureShort') }}</th>
                <th :class="cell">
                  <span class="sr-only">{{ t('explorer.nfs.actions') }}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-if="!nfs.length">
                <td :class="cell" colspan="6" class="py-6 text-center text-muted-foreground">
                  {{ t('explorer.nfs.none') }}
                </td>
              </tr>
              <tr v-for="(rule, index) in nfs" :key="rule.network" class="border-t">
                <td :class="cell" class="font-mono">{{ rule.network }}</td>
                <td :class="cell">
                  {{ rule.write ? t('explorer.nfs.readWrite') : t('explorer.nfs.readOnly') }}
                </td>
                <td :class="cell">{{ t(`explorer.nfs.squashes.${rule.squash ?? 'root'}`) }}</td>
                <td :class="cell">
                  {{ rule.async ? t('explorer.nfs.yes') : t('explorer.nfs.no') }}
                </td>
                <td :class="cell">
                  {{ rule.insecure ? t('explorer.nfs.yes') : t('explorer.nfs.no') }}
                </td>
                <td :class="cell" class="text-right whitespace-nowrap">
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    class="size-7"
                    :aria-label="t('explorer.nfs.edit', { client: rule.network })"
                    @click="editing = index"
                  >
                    <Pencil class="size-3.5" />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    class="size-7"
                    :aria-label="t('explorer.nfs.remove', { client: rule.network })"
                    @click="nfs = nfs.filter((_, at) => at !== index)"
                  >
                    <Trash2 class="size-3.5" />
                  </Button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <div class="flex flex-wrap items-center justify-between gap-2">
          <Button type="button" variant="outline" size="sm" @click="editing = -1">
            <Plus />
            {{ t('explorer.nfs.add') }}
          </Button>
          <span class="text-xs text-muted-foreground">
            {{ t('explorer.nfs.mountPath', { path: share.path }) }}
          </span>
        </div>
      </div>

      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="pending">{{ t('users.save') }}</Button>
      </div>
    </form>
    <NfsRuleDialog
      :open="editing !== null"
      :rule="editedRule"
      :taken="nfs.map((rule) => rule.network)"
      @update:open="(value) => !value && (editing = null)"
      @save="saveRule"
    />
  </ModalDialog>
</template>
