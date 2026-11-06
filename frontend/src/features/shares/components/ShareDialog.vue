<script setup lang="ts">
import { Plus, Trash2, TriangleAlert } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NfsRule, Share, ShareCompressionKind, ShareRule } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useShareActions, useSubjects, useVolumes } from '../api'

type Level = 'none' | 'deny' | 'read' | 'read_write'

/** Creates a share, or edits `share` if given. */
const props = defineProps<{ share?: Share }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useShareActions()
const notifications = useNotifications()
const { accounts, groups } = useSubjects()
const { data: volumes } = useVolumes()

const name = ref('')
const description = ref('')
const pool = ref('')
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
const error = ref<string | null>(null)
const pending = ref(false)

const editing = computed(() => props.share !== undefined)
const key = (kind: string, id: number) => `${kind}:${id}`

watch(open, (isOpen) => {
  if (!isOpen) return
  name.value = props.share?.name ?? ''
  description.value = props.share?.description ?? ''
  pool.value = props.share?.pool ?? volumes.value?.[0]?.name ?? ''
  recycleBin.value = props.share?.recycle_bin ?? true
  const settings = props.share?.settings
  quota.value = settings?.quota_bytes
    ? String(Math.round((settings.quota_bytes / GIB) * 10) / 10)
    : ''
  compression.value = settings?.compression ?? 'lz4'
  recordSize.value = settings?.record_size_bytes ?? 131072
  levels.value = Object.fromEntries(
    (props.share?.access ?? []).map((rule) => [key(rule.kind, rule.id), rule.access]),
  )
  nfs.value = (props.share?.nfs ?? []).map((rule) => ({ ...rule }))
  error.value = null
})

function rules(): ShareRule[] {
  return Object.entries(levels.value)
    .filter(([, level]) => level !== 'none')
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

async function submit(): Promise<void> {
  pending.value = true
  error.value = null
  const nfsRules = nfs.value.filter((rule) => rule.network.trim() !== '')
  try {
    if (props.share) {
      const { job } = await actions.update(props.share.id, {
        description: description.value,
        recycle_bin: recycleBin.value,
        access: rules(),
        nfs: nfsRules,
        settings: settings(),
      })
      notifications.notify(
        'success',
        t('shares.saved', { name: props.share.name }),
        job ? t('shares.savedHint') : undefined,
      )
    } else {
      const share = await actions.create({
        name: name.value.trim(),
        description: description.value || null,
        pool: pool.value,
        recycle_bin: recycleBin.value,
        access: rules(),
        nfs: nfsRules,
        settings: settings(),
      })
      notifications.notify('success', t('shares.created', { name: share.name }), share.path)
    }
    open.value = false
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="editing ? t('shares.editTitle', { name: share?.name }) : t('shares.newTitle')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="submit">
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <template v-if="!editing">
        <div class="grid gap-2">
          <Label for="share-name">{{ t('shares.name') }}</Label>
          <Input
            id="share-name"
            v-model="name"
            autocomplete="off"
            autocapitalize="none"
            spellcheck="false"
            maxlength="32"
            required
          />
          <p class="text-xs text-muted-foreground">{{ t('shares.nameHint') }}</p>
        </div>
        <div class="grid gap-2">
          <Label for="share-pool">{{ t('shares.volume') }}</Label>
          <select
            id="share-pool"
            v-model="pool"
            required
            class="h-9 rounded-md border bg-background px-3 text-sm shadow-xs"
          >
            <option v-for="volume in volumes" :key="volume.name" :value="volume.name">
              {{ volume.mount_point }} ({{ volume.name }})
            </option>
          </select>
          <p v-if="!volumes?.length" class="text-xs text-destructive">
            {{ t('shares.noVolumes') }}
          </p>
        </div>
      </template>
      <div class="grid gap-2">
        <Label for="share-description">{{ t('shares.description') }}</Label>
        <Input id="share-description" v-model="description" autocomplete="off" maxlength="200" />
      </div>

      <fieldset class="grid gap-2">
        <legend class="mb-1 text-sm font-medium">{{ t('shares.access') }}</legend>
        <p class="text-xs text-muted-foreground">{{ t('shares.accessHint') }}</p>
        <div class="grid max-h-60 gap-1 overflow-y-auto rounded-lg border p-2">
          <div
            v-for="group in groups.data.value"
            :key="key('group', group.id)"
            class="flex items-center gap-2 px-1 text-sm"
          >
            <span class="flex-1 truncate">
              {{ group.name }}
              <span class="text-xs text-muted-foreground">({{ t('shares.group') }})</span>
            </span>
            <select
              v-model="levels[key('group', group.id)]"
              class="h-8 rounded-md border bg-background px-2 text-sm"
              :aria-label="group.name"
            >
              <option :value="undefined">{{ t('shares.levels.none') }}</option>
              <option value="read">{{ t('shares.levels.read') }}</option>
              <option value="read_write">{{ t('shares.levels.read_write') }}</option>
              <option value="deny">{{ t('shares.levels.deny') }}</option>
            </select>
          </div>
          <div
            v-for="account in accounts.data.value"
            :key="key('account', account.id)"
            class="flex items-center gap-2 px-1 text-sm"
          >
            <span class="flex-1 truncate">{{ account.full_name ?? account.name }}</span>
            <select
              v-model="levels[key('account', account.id)]"
              class="h-8 rounded-md border bg-background px-2 text-sm"
              :aria-label="account.name"
            >
              <option :value="undefined">{{ t('shares.levels.none') }}</option>
              <option value="read">{{ t('shares.levels.read') }}</option>
              <option value="read_write">{{ t('shares.levels.read_write') }}</option>
              <option value="deny">{{ t('shares.levels.deny') }}</option>
            </select>
          </div>
        </div>
      </fieldset>

      <label class="flex items-start gap-2 text-sm">
        <input v-model="recycleBin" type="checkbox" class="mt-0.5 size-4 accent-primary" />
        <span>
          {{ t('shares.recycleBin') }}
          <span class="block text-xs text-muted-foreground">{{ t('shares.recycleBinHint') }}</span>
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
            <select
              id="share-compression"
              v-model="compression"
              class="h-9 rounded-md border bg-background px-3 text-sm shadow-xs"
            >
              <option v-for="kind in ['lz4', 'zstd', 'off'] as const" :key="kind" :value="kind">
                {{ t(`shares.storage.compressions.${kind}`) }}
              </option>
            </select>
          </div>
          <div class="grid gap-1">
            <Label for="share-record-size">{{ t('shares.storage.recordSize') }}</Label>
            <select
              id="share-record-size"
              v-model.number="recordSize"
              class="h-9 rounded-md border bg-background px-3 text-sm shadow-xs"
            >
              <option v-for="size in RECORD_SIZES" :key="size" :value="size">
                {{ t(`shares.storage.recordSizes.${size}`) }}
              </option>
            </select>
          </div>
        </div>
      </fieldset>

      <fieldset class="grid gap-2">
        <legend class="mb-1 text-sm font-medium">{{ t('shares.nfs') }}</legend>
        <Alert>
          <TriangleAlert class="text-amber-600 dark:text-amber-400" />
          {{ t('shares.nfsWarning') }}
        </Alert>
        <div v-for="(rule, index) in nfs" :key="index" class="flex items-center gap-2">
          <Input
            v-model="rule.network"
            :placeholder="t('shares.nfsNetwork')"
            :aria-label="t('shares.nfsNetwork')"
            autocomplete="off"
            spellcheck="false"
          />
          <label class="flex shrink-0 items-center gap-1 text-sm">
            <input v-model="rule.write" type="checkbox" class="size-4 accent-primary" />
            {{ t('shares.nfsWrite') }}
          </label>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            :aria-label="t('shares.nfsRemove')"
            @click="nfs.splice(index, 1)"
          >
            <Trash2 />
          </Button>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          class="justify-self-start"
          @click="nfs.push({ network: '', write: false })"
        >
          <Plus />
          {{ t('shares.nfsAdd') }}
        </Button>
      </fieldset>

      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="pending || (!editing && !pool)">
          {{ editing ? t('users.save') : t('shares.create') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
