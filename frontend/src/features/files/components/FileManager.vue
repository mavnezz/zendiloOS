<script setup lang="ts">
import {
  ArrowUp,
  ClipboardPaste,
  Copy,
  Download,
  Eye,
  EyeOff,
  File,
  Folder,
  FolderPlus,
  Link,
  Pencil,
  Scissors,
  Trash2,
  Upload,
  X,
} from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { ApiProblem } from '@/shared/api/client'
import type { FileEntry } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { Meter } from '@/shared/ui/meter'

import {
  downloadUrl,
  joinPath,
  previewKind,
  uploadFile,
  useFileActions,
  useFolder,
  useMyShares,
} from '../api'
import FilePreview from './FilePreview.vue'
import NameDialog from './NameDialog.vue'

/** The signed-in user's name: their recycle bin is `.recycle/<name>`. */
const props = defineProps<{ userName?: string }>()

const RECYCLE_BIN = '.recycle'

const { t } = useI18n()
const notifications = useNotifications()
const jobs = useJobsStore()
const actions = useFileActions()
const { data: shares, error: sharesError, isPending: sharesPending } = useMyShares()

const shareId = ref<number | null>(null)
const path = ref('')
const showHidden = ref(false)
const selected = ref(new Set<string>())
const { data: folder, error: folderError, isFetching } = useFolder(shareId, path)

watch(
  shares,
  (list) => {
    if (list && !list.some((share) => share.id === shareId.value)) {
      shareId.value = list[0]?.id ?? null
      path.value = ''
    }
  },
  { immediate: true },
)
watch([shareId, path], () => (selected.value = new Set()))

const share = computed(() => shares.value?.find((s) => s.id === shareId.value))
const inRecycleBin = computed(() => path.value.split('/')[0] === RECYCLE_BIN)
const ownBin = computed(() => (props.userName ? `${RECYCLE_BIN}/${props.userName}` : null))
const writable = computed(
  () => share.value?.access === 'read_write' && (folder.value?.writable ?? false),
)
/** The bin does not exist before the first deletion. */
const emptyBin = computed(
  () =>
    inRecycleBin.value &&
    folderError.value instanceof ApiProblem &&
    folderError.value.status === 404,
)

const collator = computed(
  () => new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' }),
)
const entries = computed(() =>
  (folder.value?.entries ?? [])
    .filter((entry) => showHidden.value || !entry.name.startsWith('.'))
    .sort(
      (a, b) =>
        Number(b.kind === 'directory') - Number(a.kind === 'directory') ||
        collator.value.compare(a.name, b.name),
    ),
)
const crumbs = computed(() => {
  const names = path.value ? path.value.split('/') : []
  return names.map((name, index) => ({ name, path: names.slice(0, index + 1).join('/') }))
})
const allSelected = computed(
  () => entries.value.length > 0 && entries.value.every((e) => selected.value.has(e.name)),
)
const selectedPaths = computed(() => [...selected.value].map((name) => joinPath(path.value, name)))
const single = computed(() =>
  selected.value.size === 1 ? entries.value.find((e) => selected.value.has(e.name)) : undefined,
)

function open(target: string): void {
  path.value = target
}

/** The file shown in the preview, by its path in the share. */
const previewing = ref<string | null>(null)
const previewOpen = computed({
  get: () => previewing.value !== null,
  set: (isOpen) => {
    if (!isOpen) previewing.value = null
  },
})

function up(): void {
  path.value = path.value.split('/').slice(0, -1).join('/')
}

function choose(id: number): void {
  shareId.value = id
  path.value = ''
}

function toggle(name: string): void {
  const next = new Set(selected.value)
  if (!next.delete(name)) next.add(name)
  selected.value = next
}

function toggleAll(): void {
  selected.value = allSelected.value ? new Set() : new Set(entries.value.map((e) => e.name))
}

function fail(failure: unknown): void {
  notifications.notify('error', problemMessage(failure))
}

// Copying and moving between folders and shares.
const clipboard = ref<{ mode: 'copy' | 'move'; share: number; paths: string[] } | null>(null)

function take(mode: 'copy' | 'move'): void {
  if (shareId.value === null || selected.value.size === 0) return
  clipboard.value = { mode, share: shareId.value, paths: selectedPaths.value }
  selected.value = new Set()
}

async function paste(): Promise<void> {
  const taken = clipboard.value
  if (!taken || shareId.value === null) return
  try {
    await actions.transfer(taken.share, {
      mode: taken.mode,
      paths: taken.paths,
      target_share: shareId.value,
      target_path: path.value,
    })
    notifications.notify(
      'info',
      t(`files.started.${taken.mode}`, { count: taken.paths.length }, taken.paths.length),
    )
    if (taken.mode === 'move') clipboard.value = null
  } catch (failure) {
    fail(failure)
  }
}

// When a copy, move or delete is over, the folder may look different.
watch(
  () => jobs.list.filter((job) => job.kind.startsWith('files.') && job.state !== 'running').length,
  () => void actions.refresh(),
)

// Deleting.
const deleting = ref(false)
const forGood = computed(() => !share.value?.recycle_bin || inRecycleBin.value)

async function remove(): Promise<void> {
  if (shareId.value === null) return
  const paths = selectedPaths.value
  try {
    await actions.remove(shareId.value, paths)
    notifications.notify('info', t('files.started.delete', { count: paths.length }, paths.length))
    selected.value = new Set()
  } catch (failure) {
    fail(failure)
  }
}

// New folders and new names.
const naming = ref<'folder' | 'rename' | null>(null)
const namingError = ref<string | null>(null)

async function named(name: string): Promise<void> {
  if (shareId.value === null) return
  namingError.value = null
  try {
    if (naming.value === 'folder') {
      await actions.createFolder(shareId.value, joinPath(path.value, name))
    } else if (single.value) {
      await actions.rename(shareId.value, joinPath(path.value, single.value.name), name)
      selected.value = new Set()
    }
    naming.value = null
  } catch (failure) {
    namingError.value = problemMessage(failure)
  }
}

// Uploads.
interface Transfer {
  id: number
  name: string
  progress: number
  error?: string
  controller: AbortController
}
const uploads = ref<Transfer[]>([])
let nextUpload = 0
const picker = ref<HTMLInputElement | null>(null)
const dragging = ref(false)
const conflicts = ref<{ files: File[]; taken: Set<string> } | null>(null)
const asking = ref(false)
// Closing the question without confirming skips the taken names. Checked
// after the dialog's events, which come in no fixed order.
watch(asking, (open) => {
  if (!open) resolveConflicts(false)
})

function upload(files: File[]): void {
  if (!writable.value || files.length === 0) return
  const existing = new Set(folder.value?.entries.map((e) => e.name))
  const taken = new Set(files.filter((f) => existing.has(f.name)).map((f) => f.name))
  if (taken.size > 0) {
    conflicts.value = { files, taken }
    asking.value = true
  } else {
    for (const file of files) void send(file, false)
  }
}

function resolveConflicts(replace: boolean): void {
  const pending = conflicts.value
  conflicts.value = null
  if (!pending) return
  for (const file of pending.files) {
    const isTaken = pending.taken.has(file.name)
    if (!isTaken || replace) void send(file, isTaken)
  }
}

async function send(file: File, replace: boolean): Promise<void> {
  if (shareId.value === null) return
  const id = nextUpload++
  const controller = new AbortController()
  uploads.value.push({ id, name: file.name, progress: 0, controller })
  const tracked = () => uploads.value.find((u) => u.id === id)
  const forget = () => (uploads.value = uploads.value.filter((u) => u.id !== id))
  try {
    await uploadFile(shareId.value, path.value, file, {
      replace,
      signal: controller.signal,
      onProgress: (fraction) => {
        const current = tracked()
        if (current) current.progress = fraction
      },
    })
    forget()
    await actions.refresh()
  } catch (failure) {
    const current = tracked()
    if (failure instanceof ApiProblem && failure.code === 'aborted') forget()
    else if (current) current.error = problemMessage(failure)
  }
}

function dismiss(transfer: Transfer): void {
  transfer.controller.abort()
  uploads.value = uploads.value.filter((u) => u.id !== transfer.id)
}

function picked(event: Event): void {
  const input = event.target as HTMLInputElement
  upload([...(input.files ?? [])])
  input.value = ''
}

function dropped(event: DragEvent): void {
  dragging.value = false
  upload([...(event.dataTransfer?.files ?? [])])
}

function icon(entry: FileEntry) {
  return entry.kind === 'directory' ? Folder : entry.kind === 'symlink' ? Link : File
}
</script>

<!-- The contents of the user's shares, with the user's own permissions. -->
<template>
  <div class="flex h-full min-h-0 flex-col @3xl:flex-row">
    <nav
      class="flex shrink-0 gap-2 border-b p-2 @3xl:w-52 @3xl:flex-col @3xl:border-r @3xl:border-b-0"
      :aria-label="t('files.shares')"
    >
      <select
        v-if="shares?.length"
        class="h-9 w-full rounded-md border bg-background px-3 text-sm @3xl:hidden"
        :aria-label="t('files.shares')"
        :value="shareId ?? undefined"
        @change="choose(Number(($event.target as HTMLSelectElement).value))"
      >
        <option v-for="item in shares" :key="item.id" :value="item.id">{{ item.name }}</option>
      </select>
      <button
        v-for="item in shares"
        :key="item.id"
        type="button"
        class="hidden items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent @3xl:flex"
        :class="item.id === shareId ? 'bg-accent font-medium' : ''"
        :aria-current="item.id === shareId ? 'true' : undefined"
        @click="choose(item.id)"
      >
        <Folder class="size-4 shrink-0 text-primary" />
        <span class="truncate">{{ item.name }}</span>
      </button>
    </nav>

    <div
      class="relative flex min-h-0 min-w-0 flex-1 flex-col"
      @dragover.prevent="dragging = writable"
      @dragleave="dragging = false"
      @drop.prevent="dropped"
    >
      <Alert v-if="sharesError" variant="destructive" class="m-4">
        {{ problemMessage(sharesError) }}
      </Alert>
      <p v-else-if="!sharesPending && !shares?.length" class="p-6 text-sm text-muted-foreground">
        {{ t('files.noShares') }}
      </p>

      <template v-else-if="share">
        <div class="flex flex-wrap items-center gap-1 border-b p-2">
          <Button
            variant="ghost"
            size="icon"
            :disabled="!path"
            :title="t('files.up')"
            :aria-label="t('files.up')"
            @click="up"
          >
            <ArrowUp />
          </Button>
          <ol
            class="flex min-w-0 flex-1 flex-wrap items-center gap-1 text-sm"
            :aria-label="t('files.location')"
          >
            <li>
              <button
                type="button"
                class="rounded px-1 font-medium hover:bg-accent"
                @click="open('')"
              >
                {{ share.name }}
              </button>
            </li>
            <li v-for="crumb in crumbs" :key="crumb.path" class="flex min-w-0 items-center gap-1">
              <span class="text-muted-foreground">/</span>
              <button
                type="button"
                class="truncate rounded px-1 hover:bg-accent"
                @click="open(crumb.path)"
              >
                {{ crumb.name === RECYCLE_BIN ? t('files.recycleBin') : crumb.name }}
              </button>
            </li>
          </ol>
          <Badge v-if="share.access === 'read'" variant="secondary">{{
            t('files.readOnly')
          }}</Badge>
          <Button
            variant="ghost"
            size="icon"
            :title="showHidden ? t('files.hideHidden') : t('files.showHidden')"
            :aria-label="showHidden ? t('files.hideHidden') : t('files.showHidden')"
            :aria-pressed="showHidden"
            @click="showHidden = !showHidden"
          >
            <EyeOff v-if="showHidden" />
            <Eye v-else />
          </Button>
          <Button
            v-if="share.recycle_bin && ownBin && !inRecycleBin"
            variant="ghost"
            size="icon"
            :title="t('files.recycleBin')"
            :aria-label="t('files.recycleBin')"
            @click="open(ownBin)"
          >
            <Trash2 />
          </Button>
        </div>

        <div class="flex flex-wrap items-center gap-1.5 border-b px-2 py-1.5">
          <Button size="sm" variant="outline" :disabled="!writable" @click="picker?.click()">
            <Upload />
            {{ t('files.upload') }}
          </Button>
          <input ref="picker" type="file" multiple class="hidden" @change="picked" />
          <Button
            size="sm"
            variant="outline"
            :disabled="!writable"
            @click="((naming = 'folder'), (namingError = null))"
          >
            <FolderPlus />
            {{ t('files.newFolder') }}
          </Button>
          <template v-if="selected.size">
            <span class="mx-1 text-xs text-muted-foreground">
              {{ t('files.selected', { count: selected.size }, selected.size) }}
            </span>
            <Button
              v-if="single?.kind === 'file'"
              size="sm"
              variant="ghost"
              as="a"
              :href="downloadUrl(share.id, joinPath(path, single.name))"
              download
            >
              <Download />
              {{ t('files.download') }}
            </Button>
            <Button
              v-if="single"
              size="sm"
              variant="ghost"
              :disabled="!writable"
              @click="((naming = 'rename'), (namingError = null))"
            >
              <Pencil />
              {{ t('files.rename') }}
            </Button>
            <Button size="sm" variant="ghost" @click="take('copy')">
              <Copy />
              {{ t('files.copy') }}
            </Button>
            <Button size="sm" variant="ghost" :disabled="!writable" @click="take('move')">
              <Scissors />
              {{ t('files.cut') }}
            </Button>
            <Button size="sm" variant="ghost" :disabled="!writable" @click="deleting = true">
              <Trash2 />
              {{ t('files.delete') }}
            </Button>
          </template>
          <template v-if="clipboard">
            <Button size="sm" :disabled="!writable" @click="paste">
              <ClipboardPaste />
              {{
                t(
                  `files.paste.${clipboard.mode}`,
                  { count: clipboard.paths.length },
                  clipboard.paths.length,
                )
              }}
            </Button>
            <Button
              size="icon"
              variant="ghost"
              class="size-8"
              :title="t('files.clearClipboard')"
              :aria-label="t('files.clearClipboard')"
              @click="clipboard = null"
            >
              <X />
            </Button>
          </template>
        </div>

        <div class="min-h-0 flex-1 overflow-auto" :aria-busy="isFetching">
          <p v-if="emptyBin" class="p-6 text-sm text-muted-foreground">{{ t('files.binEmpty') }}</p>
          <Alert v-else-if="folderError" variant="destructive" class="m-4">
            {{ problemMessage(folderError) }}
          </Alert>
          <template v-else-if="folder">
            <table class="w-full text-sm">
              <thead class="sticky top-0 bg-background text-left text-xs text-muted-foreground">
                <tr class="border-b">
                  <th class="w-8 p-2">
                    <input
                      type="checkbox"
                      class="size-4 accent-primary"
                      :checked="allSelected"
                      :aria-label="t('files.selectAll')"
                      @change="toggleAll"
                    />
                  </th>
                  <th class="p-2 font-medium">{{ t('files.name') }}</th>
                  <th class="hidden p-2 text-right font-medium @xl:table-cell">
                    {{ t('files.size') }}
                  </th>
                  <th class="hidden p-2 font-medium @2xl:table-cell">{{ t('files.modified') }}</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="entry in entries"
                  :key="entry.name"
                  class="border-b last:border-b-0 hover:bg-accent/50"
                  :class="selected.has(entry.name) ? 'bg-accent/60' : ''"
                >
                  <td class="p-2">
                    <input
                      type="checkbox"
                      class="size-4 accent-primary"
                      :checked="selected.has(entry.name)"
                      :aria-label="entry.name"
                      @change="toggle(entry.name)"
                    />
                  </td>
                  <td class="max-w-0 p-2">
                    <button
                      v-if="entry.kind === 'directory'"
                      type="button"
                      class="flex w-full min-w-0 items-center gap-2 text-left hover:underline"
                      @click="open(joinPath(path, entry.name))"
                    >
                      <component :is="icon(entry)" class="size-4 shrink-0 text-primary" />
                      <span class="truncate">{{ entry.name }}</span>
                    </button>
                    <button
                      v-else-if="entry.kind === 'file' && previewKind(entry.name)"
                      type="button"
                      class="flex w-full min-w-0 items-center gap-2 text-left hover:underline"
                      :title="t('files.preview')"
                      @click="previewing = joinPath(path, entry.name)"
                    >
                      <component :is="icon(entry)" class="size-4 shrink-0 text-muted-foreground" />
                      <span class="truncate">{{ entry.name }}</span>
                    </button>
                    <a
                      v-else
                      class="flex min-w-0 items-center gap-2 hover:underline"
                      :href="downloadUrl(share.id, joinPath(path, entry.name))"
                      download
                    >
                      <component :is="icon(entry)" class="size-4 shrink-0 text-muted-foreground" />
                      <span class="truncate">{{ entry.name }}</span>
                    </a>
                  </td>
                  <td class="hidden p-2 text-right whitespace-nowrap tabular-nums @xl:table-cell">
                    {{ entry.kind === 'file' ? formatBytes(entry.size) : '' }}
                  </td>
                  <td class="hidden p-2 whitespace-nowrap text-muted-foreground @2xl:table-cell">
                    {{ formatDateTime(entry.modified * 1000) }}
                  </td>
                </tr>
              </tbody>
            </table>
            <p v-if="!entries.length" class="p-6 text-sm text-muted-foreground">
              {{ inRecycleBin ? t('files.binEmpty') : t('files.empty') }}
            </p>
            <p v-if="folder.truncated" class="p-2 text-xs text-muted-foreground">
              {{ t('files.truncated', { count: folder.entries.length }) }}
            </p>
          </template>
        </div>

        <ul v-if="uploads.length" class="grid max-h-40 gap-2 overflow-y-auto border-t p-2">
          <li v-for="item in uploads" :key="item.id" class="flex items-center gap-2 text-sm">
            <Upload class="size-4 shrink-0 text-muted-foreground" />
            <span class="w-40 truncate @xl:w-60">{{ item.name }}</span>
            <span v-if="item.error" class="flex-1 truncate text-destructive">{{ item.error }}</span>
            <Meter
              v-else
              class="flex-1"
              kind="progress"
              :value="item.progress"
              :label="t('files.uploading', { name: item.name })"
            />
            <Button
              size="icon"
              variant="ghost"
              class="size-7"
              :title="item.error ? t('files.dismiss') : t('files.cancelUpload')"
              :aria-label="item.error ? t('files.dismiss') : t('files.cancelUpload')"
              @click="dismiss(item)"
            >
              <X />
            </Button>
          </li>
        </ul>

        <div
          v-if="dragging"
          class="pointer-events-none absolute inset-2 grid place-items-center rounded-xl border-2 border-dashed border-primary bg-primary/5 text-sm font-medium text-primary"
        >
          {{ t('files.dropHere') }}
        </div>
      </template>
    </div>

    <FilePreview v-if="share" v-model:open="previewOpen" :share="share.id" :path="previewing" />
    <NameDialog
      :open="naming !== null"
      :title="
        naming === 'folder' ? t('files.newFolder') : t('files.renameTitle', { name: single?.name })
      "
      :initial="naming === 'rename' ? (single?.name ?? '') : ''"
      :submit-label="naming === 'folder' ? t('files.create') : t('files.rename')"
      :error="namingError"
      @update:open="(value) => !value && (naming = null)"
      @submit="named"
    />
    <ConfirmDialog
      v-model:open="deleting"
      :title="forGood ? t('files.deleteForGoodTitle') : t('files.deleteTitle')"
      :description="
        forGood
          ? t('files.deleteForGood', { count: selected.size }, selected.size)
          : t('files.deleteToBin', { count: selected.size }, selected.size)
      "
      :confirm-label="t('files.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="remove"
    />
    <ConfirmDialog
      v-model:open="asking"
      :title="t('files.replaceTitle')"
      :description="
        t('files.replaceDescription', { names: [...(conflicts?.taken ?? [])].join(', ') })
      "
      :confirm-label="t('files.replace')"
      :cancel-label="t('files.skip')"
      @confirm="resolveConflicts(true)"
    />
  </div>
</template>
