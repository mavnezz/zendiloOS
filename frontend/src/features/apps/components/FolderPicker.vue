<script setup lang="ts">
import { ChevronRight, Folder, FolderInput, LoaderCircle } from 'lucide-vue-next'
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'

import { listFolders, makeFolder, useShareList } from '../api'

/** `value`: the folder picked before, `share/folder/…`; `writable`: only shares the app may write to. */
const props = defineProps<{ value: string; writable?: boolean }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ pick: [path: string] }>()

const { t } = useI18n()
const { data: allShares } = useShareList()
/** The shares to pick from: where the app writes, those the administrator may write. */
const shares = computed(() =>
  (allShares.value ?? []).filter((share) => !props.writable || share.writable),
)

/** What is known of one folder: its folders once read. */
interface Known {
  names: string[] | null
  writable: boolean
  loading: boolean
  error: string | null
}
const known = reactive(new Map<string, Known>())
const expanded = reactive(new Set<string>())
const rootOpen = ref(true)
const selected = ref('')
const creating = ref<{ parent: string; name: string } | null>(null)
const failure = ref<string | null>(null)
const busy = ref(false)

const shareIds = computed(
  () => new Map((shares.value ?? []).map((share) => [share.name, share.id])),
)

/** The share and the folder in it of a path such as `media/Films`. */
function split(path: string): { share: string; folder: string } {
  const [share = '', ...rest] = path.split('/')
  return { share, folder: rest.join('/') }
}

async function load(path: string): Promise<void> {
  const { share, folder } = split(path)
  const id = shareIds.value.get(share)
  if (id === undefined) return
  if (!known.has(path))
    known.set(path, { names: null, writable: false, loading: false, error: null })
  // The reactive entry, so that what is read shows.
  const entry = known.get(path)!
  entry.loading = true
  entry.error = null
  try {
    const read = await listFolders(id, folder)
    entry.names = read.names
    entry.writable = read.writable
  } catch (problem) {
    entry.names = []
    entry.error = problemMessage(problem)
  } finally {
    entry.loading = false
  }
}

async function toggle(path: string): Promise<void> {
  if (expanded.has(path)) {
    expanded.delete(path)
    return
  }
  expanded.add(path)
  if (!known.get(path)?.names) await load(path)
}

/** The rows shown: each share, and below an open folder its folders. */
interface Row {
  path: string
  name: string
  depth: number
  open: boolean
  /** Folders inside, as far as known. */
  more: boolean
}
const rows = computed<Row[]>(() => {
  const out: Row[] = []
  const add = (path: string, name: string, depth: number) => {
    const entry = known.get(path)
    const open = expanded.has(path)
    out.push({ path, name, depth, open, more: !entry?.names || entry.names.length > 0 })
    if (!open) return
    for (const child of entry?.names ?? []) add(`${path}/${child}`, child, depth + 1)
  }
  if (rootOpen.value) for (const share of shares.value ?? []) add(share.name, share.name, 1)
  return out
})

/** Fresh whenever the dialog opens, with the folder picked before shown. */
watch(open, async (isOpen) => {
  if (!isOpen) return
  known.clear()
  expanded.clear()
  rootOpen.value = true
  creating.value = null
  failure.value = null
  selected.value = props.value
  const parts = props.value.split('/').filter(Boolean)
  for (let end = 1; end < parts.length; end++) await toggle(parts.slice(0, end).join('/'))
})

async function refresh(): Promise<void> {
  failure.value = null
  creating.value = null
  const open = [...expanded]
  known.clear()
  for (const path of open) await load(path)
}

const canCreate = computed(() => {
  if (!selected.value || busy.value) return false
  const entry = known.get(selected.value)
  return !entry || entry.writable || !entry.names
})

async function startFolder(): Promise<void> {
  const parent = selected.value
  if (!parent) return
  failure.value = null
  if (!expanded.has(parent)) await toggle(parent)
  if (known.get(parent)?.error) return
  if (!known.get(parent)?.writable) {
    failure.value = t('appStore.folderReadOnly')
    return
  }
  creating.value = { parent, name: t('appStore.newFolder') }
  await nextTick()
  const element = document.getElementById('new-folder-name')
  if (element instanceof HTMLInputElement) {
    element.focus()
    element.select()
  }
}

async function finishFolder(): Promise<void> {
  const draft = creating.value
  if (!draft) return
  const name = draft.name.trim()
  if (!name) {
    creating.value = null
    return
  }
  const { share, folder } = split(draft.parent)
  const id = shareIds.value.get(share)
  if (id === undefined) return
  busy.value = true
  failure.value = null
  try {
    await makeFolder(id, folder ? `${folder}/${name}` : name)
    creating.value = null
    await load(draft.parent)
    selected.value = `${draft.parent}/${name}`
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}

function confirm(): void {
  if (!selected.value) return
  emit('pick', selected.value)
  open.value = false
}

const indent = (depth: number) => ({ paddingLeft: `${depth * 1.25 + 0.25}rem` })
</script>

<!-- A folder of a share for an app: the shares, opened folder by folder. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('appStore.pickFolder')"
    :close-label="t('desktop.close')"
  >
    <div class="flex gap-2">
      <Button
        type="button"
        variant="outline"
        size="sm"
        class="border-primary text-primary"
        :disabled="!canCreate"
        @click="startFolder"
        >{{ t('appStore.newFolder') }}</Button
      >
      <Button type="button" variant="outline" size="sm" :disabled="busy" @click="refresh">{{
        t('appStore.refresh')
      }}</Button>
    </div>

    <div
      role="tree"
      :aria-label="t('appStore.sharedFolders')"
      class="grid max-h-80 min-h-48 content-start overflow-y-auto text-sm"
    >
      <button
        type="button"
        class="flex h-8 items-center gap-1.5 rounded-md px-1 text-left font-semibold hover:bg-foreground/5"
        :aria-expanded="rootOpen"
        @click="rootOpen = !rootOpen"
      >
        <ChevronRight class="size-4 transition-transform" :class="rootOpen && 'rotate-90'" />
        {{ t('appStore.sharedFolders') }}
      </button>
      <template v-for="row in rows" :key="row.path">
        <div
          role="treeitem"
          :aria-selected="selected === row.path"
          :aria-expanded="row.more ? row.open : undefined"
          class="flex h-8 items-center gap-1 rounded-md"
          :class="selected === row.path ? 'bg-primary/10 text-primary' : 'hover:bg-foreground/5'"
          :style="indent(row.depth)"
        >
          <button
            type="button"
            class="grid size-5 shrink-0 place-items-center rounded text-muted-foreground"
            :class="!row.more && 'invisible'"
            :aria-label="row.open ? t('appStore.collapse') : t('appStore.expand')"
            :tabindex="row.more ? 0 : -1"
            @click="toggle(row.path)"
          >
            <LoaderCircle v-if="known.get(row.path)?.loading" class="size-3.5 animate-spin" />
            <ChevronRight
              v-else
              class="size-4 transition-transform"
              :class="row.open && 'rotate-90'"
            />
          </button>
          <button
            type="button"
            class="flex min-w-0 flex-1 items-center gap-2 self-stretch text-left"
            @click="selected = row.path"
            @dblclick="toggle(row.path)"
          >
            <FolderInput v-if="row.depth === 1" class="size-4 shrink-0 text-sky-500" />
            <Folder v-else class="size-4 shrink-0 fill-sky-400/80 text-sky-500" />
            <span class="truncate">{{ row.name }}</span>
          </button>
        </div>
        <p
          v-if="row.open && known.get(row.path)?.error"
          class="py-1 text-xs text-muted-foreground"
          :style="indent(row.depth + 1)"
        >
          {{ t('appStore.noFolderAccess') }}
        </p>
        <form
          v-if="creating && creating.parent === row.path"
          class="flex h-9 items-center gap-2 pr-1"
          :style="indent(row.depth + 1)"
          @submit.prevent="finishFolder"
        >
          <Folder class="ml-6 size-4 shrink-0 fill-sky-400/80 text-sky-500" />
          <Input
            id="new-folder-name"
            v-model="creating.name"
            class="h-7"
            :aria-label="t('appStore.folderName')"
            :disabled="busy"
            @keydown.esc.stop.prevent="creating = null"
          />
          <Button type="submit" size="sm" :disabled="busy">{{ t('appStore.create') }}</Button>
        </form>
      </template>
    </div>

    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>

    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button type="button" :disabled="!selected" @click="confirm">{{
        t('appStore.confirm')
      }}</Button>
    </template>
  </ModalDialog>
</template>
