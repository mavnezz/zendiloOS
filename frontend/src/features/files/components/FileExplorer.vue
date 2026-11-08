<script setup lang="ts">
import { useQueries, useQueryClient } from '@tanstack/vue-query'
import {
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  ClipboardPaste,
  Copy,
  Download,
  Eye,
  EyeOff,
  FolderOpen,
  FolderPlus,
  FolderSearch,
  History,
  Info,
  Network,
  PanelLeft,
  Pencil,
  Plug,
  RotateCcw,
  Scissors,
  Search,
  Star,
  Trash2,
  Unplug,
  Upload,
  X,
} from 'lucide-vue-next'
import { computed, onBeforeUnmount, provide, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { ApiProblem, api, unwrap } from '@/shared/api/client'
import type { FileEntry, Folder, FoundFile } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatDateTime } from '@/shared/lib/format'
import { useNotifications } from '@/shared/lib/notifications'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ContextMenu, type MenuAction } from '@/shared/ui/context-menu'
import { Meter } from '@/shared/ui/meter'

import {
  archiveUrl,
  downloadUrl,
  listFolder,
  previewKind,
  restoreFromSnapshot,
  targetOf,
  uploadFile,
  useDriveActions,
  useFavourites,
  useFileActions,
  useFileVolumes,
  useFolder,
  useFolderSearch,
  useMyShares,
  useRemovableDrives,
  useSetFavourites,
  useShareSnapshots,
} from '../api'
import {
  type ActionId,
  actionsFor,
  type EntryInfo,
  extension,
  type Item,
  type Situation,
  shortcut,
  type SortKey,
  sortItems,
} from '../lib/actions'
import {
  folderAt,
  HOME,
  inBin,
  isDrive,
  joinPath,
  type Location,
  ownBin,
  parentPath,
  type Place,
  RECYCLE_BIN,
  useHistory,
  withinBin,
} from '../lib/location'
import { EXPLORER, folderItem } from '../lib/tree'
import { useShareActions, useShares } from '../sharing'
import DetailsView from './DetailsView.vue'
import EntryPropertiesDialog from './EntryPropertiesDialog.vue'
import FilePreview from './FilePreview.vue'
import NameDialog from './NameDialog.vue'
import NavigationTree from './NavigationTree.vue'
import PreviousVersionsDialog from './PreviousVersionsDialog.vue'
import DeleteFolderDialog from './sharing/DeleteFolderDialog.vue'
import FolderPropertiesDialog from './sharing/FolderPropertiesDialog.vue'
import SnapshotsDialog from './sharing/SnapshotsDialog.vue'

/**
 * The files of the NAS as the signed-in person may see them, with their own
 * permissions. `canManage`: they manage shares (`shares.manage`), so make,
 * share and delete folders directly on the volumes. `canManageStorage`:
 * they manage storage (`storage.manage`), so see, use and eject the
 * removable drives plugged in.
 */
const props = defineProps<{
  userName?: string
  accountId?: number
  canManage?: boolean
  canManageStorage?: boolean
}>()

const { t } = useI18n()
const notifications = useNotifications()
const jobs = useJobsStore()
const client = useQueryClient()
const actions = useFileActions()
const shareActions = useShareActions()
const { data: places, error: placesError } = useMyShares()
const { data: volumes, error: volumesError, isPending: volumesPending } = useFileVolumes()
const { data: favourites } = useFavourites()
const setFavourites = useSetFavourites()
const managed = useShares({ enabled: computed(() => props.canManage === true) })
const { data: drives } = useRemovableDrives(computed(() => props.canManageStorage === true))
const driveActions = useDriveActions()

const history = useHistory({ kind: 'bin' })
const location = history.current
const started = ref(false)
const bin = computed(() => ownBin(props.userName))
const hasHome = computed(() => (places.value ?? []).some((place) => place.home))

/** Where the explorer opens: the personal folder, else the first folder or volume. */
function startLocation(): Location {
  const first = (volumes.value ?? []).flatMap((v) => v.folders).find((f) => f.access)
  if (hasHome.value) return folderAt(HOME)
  if (first) return folderAt(first.id)
  const volume = volumes.value?.[0]
  return volume ? { kind: 'volume', volume: volume.volume } : { kind: 'bin' }
}
watch(
  [places, volumes],
  ([placeList, volumeList]) => {
    if (started.value || !placeList || !volumeList) return
    started.value = true
    if (placeList.length || volumeList.length) history.replace(startLocation())
  },
  { immediate: true },
)

/** The folder on a volume (or personal folder) `id`, as the person may use it; never a drive. */
function place(id: Place) {
  return isDrive(id) ? undefined : places.value?.find((p) => p.id === id)
}
function volumeOf(id: Place) {
  return isDrive(id) ? undefined : volumes.value?.find((v) => v.folders.some((f) => f.id === id))
}
/** The removable drive `id`, as last listed. */
function driveOf(id: Place) {
  return isDrive(id) ? drives.value?.find((d) => d.id === id) : undefined
}
function folderName(id: Place): string {
  if (isDrive(id)) return driveOf(id)?.label || id
  if (id === HOME) return t('files.home')
  return place(id)?.name ?? volumeOf(id)?.folders.find((f) => f.id === id)?.name ?? `#${id}`
}
/** What the person may do in a place: a drive is the administrator's while mounted. */
function accessOf(id: Place): 'read' | 'read_write' | undefined {
  if (!isDrive(id)) return place(id)?.access
  const drive = driveOf(id)
  if (drive?.state !== 'mounted') return undefined
  return drive.read_only ? 'read' : 'read_write'
}

// A drive ejected or pulled out while looked into: back to where the explorer starts.
watch(drives, (list) => {
  const at = location.value
  if (at.kind !== 'folder' || !isDrive(at.share) || !list) return
  if (list.find((d) => d.id === at.share)?.state !== 'mounted') history.replace(startLocation())
})

// What is shown: a folder, or a folder in one of the person's bins.
const binShare = computed(() =>
  location.value.kind === 'bin' && bin.value ? (location.value.share ?? null) : null,
)
const shareRef = computed(() =>
  location.value.kind === 'folder' ? location.value.share : binShare.value,
)
const pathRef = computed(() => {
  const at = location.value
  if (at.kind === 'folder') return at.path
  return binShare.value !== null && bin.value && at.kind === 'bin'
    ? joinPath(bin.value, at.path ?? '')
    : ''
})
const snapshotRef = computed(() =>
  location.value.kind === 'folder' ? location.value.snapshot : null,
)
const { data: folder, error: folderError, isFetching } = useFolder(shareRef, pathRef, snapshotRef)
const { data: snapshots } = useShareSnapshots(
  computed(() => {
    const share = shareRef.value
    return share !== null && !isDrive(share) && share !== HOME ? share : null
  }),
)

const showHidden = ref(false)
const search = ref('')
const sort = ref<{ key: SortKey; descending: boolean }>({ key: 'name', descending: false })

/** Typing pauses this long before the folder is searched on the NAS. */
const SEARCH_DELAY_MS = 300
// What is typed filters the entries shown at once; in a folder, the NAS
// then looks through everything below it, and its findings take their place.
const searchText = ref('')
let searchTimer: ReturnType<typeof setTimeout> | undefined
watch(search, (value) => {
  clearTimeout(searchTimer)
  const text = value.trim()
  if (!text) searchText.value = ''
  else searchTimer = setTimeout(() => (searchText.value = text), SEARCH_DELAY_MS)
})
onBeforeUnmount(() => clearTimeout(searchTimer))
const searchShare = computed(() => (location.value.kind === 'folder' ? location.value.share : null))
const found = useFolderSearch(
  searchShare,
  pathRef,
  snapshotRef,
  computed(() => (location.value.kind === 'folder' ? searchText.value : '')),
  showHidden,
)
/** What the NAS found below the folder shown, while a search is typed. */
const results = computed(() =>
  location.value.kind === 'folder' && searchText.value ? found.data.value : undefined,
)
const searchError = computed(() =>
  location.value.kind === 'folder' && searchText.value ? found.error.value : null,
)
const searchBusy = computed(
  () => location.value.kind === 'folder' && searchText.value !== '' && found.isFetching.value,
)

/** What the search line says: under way, failed, found, or stopped early. */
const searchStatus = computed(() => {
  if (searchError.value) {
    return t('explorer.search.failed', { problem: problemMessage(searchError.value) })
  }
  const done = results.value
  if (!done) return t('explorer.search.busy', { text: searchText.value })
  const count = done.entries.length
  if (done.stopped === 'results') return t('explorer.search.stoppedResults', { count })
  if (done.stopped === 'time') return t('explorer.search.stoppedTime', { count }, count)
  return t('explorer.search.found', { count, text: searchText.value }, count)
})

function foundItem(share: Place, entry: FoundFile): Item {
  return {
    key: `${share}:${entry.path}`,
    name: entry.name,
    kind: entry.kind,
    size: entry.kind === 'file' ? entry.size : null,
    modified: entry.modified,
    unreadable: entry.unreadable,
    share,
    path: entry.path,
    origin: parentPath(entry.path) || folderName(share),
  }
}

function entryItem(share: Place, parent: string, entry: FileEntry, origin?: string): Item {
  const path = joinPath(parent, entry.name)
  return {
    key: `${share}:${path}`,
    name: entry.name,
    kind: entry.kind,
    size: entry.kind === 'file' ? entry.size : null,
    modified: entry.modified,
    unreadable: entry.unreadable,
    share,
    path,
    origin,
  }
}

// The recycle bins: the person's own in every folder that has one.
const binPlaces = computed(() =>
  location.value.kind === 'bin' && location.value.share === undefined && bin.value
    ? (places.value ?? []).filter((p) => p.recycle_bin)
    : [],
)
const bins = useQueries({
  queries: computed(() =>
    binPlaces.value.map((p) => ({
      queryKey: ['files', 'folder', p.id, bin.value, null],
      queryFn: async (): Promise<Folder> => {
        try {
          return await unwrap(
            api.GET('/api/v1/files/shares/{id}/folder', {
              params: { path: { id: p.id }, query: { path: bin.value ?? '' } },
            }),
          )
        } catch (failure) {
          // A bin is made with the first deletion.
          if (failure instanceof ApiProblem && failure.status === 404) {
            return { path: bin.value ?? '', entries: [], truncated: false, writable: true }
          }
          throw failure
        }
      },
      retry: false,
    })),
  ),
})

const allItems = computed<Item[]>(() => {
  const at = location.value
  if (at.kind === 'volume') {
    const volume = volumes.value?.find((v) => v.volume === at.volume)
    return (volume?.folders ?? []).map((f) => ({
      ...folderItem(f.id, '', f.name),
      volumeFolder: { shared: f.shared, access: f.access ?? null },
    }))
  }
  if (at.kind === 'bin' && at.share !== undefined) {
    const share = at.share
    const where = at.path ? `${folderName(share)}/${at.path}` : folderName(share)
    return (folder.value?.entries ?? []).map((entry) =>
      entryItem(share, pathRef.value, entry, where),
    )
  }
  if (at.kind === 'bin') {
    return binPlaces.value.flatMap((p, index) =>
      (bins.value[index]?.data?.entries ?? []).map((entry) =>
        entryItem(p.id, bin.value ?? '', entry, folderName(p.id)),
      ),
    )
  }
  return (folder.value?.entries ?? []).map((entry) => entryItem(at.share, at.path, entry))
})
const items = computed(() => {
  const at = location.value
  if (at.kind === 'folder' && results.value) {
    return sortItems(
      results.value.entries.map((entry) => foundItem(at.share, entry)),
      sort.value.key,
      sort.value.descending,
    )
  }
  const needle = search.value.trim().toLowerCase()
  return sortItems(
    allItems.value
      .filter((item) => showHidden.value || !item.name.startsWith('.'))
      .filter((item) => !needle || item.name.toLowerCase().includes(needle)),
    sort.value.key,
    sort.value.descending,
  )
})

// Choosing entries.
const selected = ref(new Set<string>())
let anchor: string | null = null
/** An entry to choose where the explorer goes next: one a search found, in its folder. */
let revealing: string | null = null
watch(location, () => {
  selected.value = revealing ? new Set([revealing]) : new Set()
  anchor = revealing
  revealing = null
  search.value = ''
})
const chosen = computed(() => items.value.filter((i) => !i.unreadable && selected.value.has(i.key)))

function pick(item: Item, event: MouseEvent): void {
  if (event.shiftKey && anchor) {
    const keys = items.value.filter((i) => !i.unreadable).map((i) => i.key)
    const ends = [keys.indexOf(anchor), keys.indexOf(item.key)].sort((a, b) => a - b)
    selected.value = new Set(keys.slice(ends[0], (ends[1] ?? 0) + 1))
    return
  }
  if (event.ctrlKey || event.metaKey) toggle(item)
  else selected.value = new Set([item.key])
  anchor = item.key
}
function toggle(item: Item): void {
  const next = new Set(selected.value)
  if (!next.delete(item.key)) next.add(item.key)
  selected.value = next
  anchor = item.key
}
function toggleAll(): void {
  const usable = items.value.filter((i) => !i.unreadable)
  const all = usable.length > 0 && usable.every((i) => selected.value.has(i.key))
  selected.value = all ? new Set() : new Set(usable.map((i) => i.key))
}

// What the actions are about: the selection where the explorer is, or a
// node of the tree that was right-clicked.
const aimed = ref<Item | null>(null)
const clipboard = ref<{ mode: 'copy' | 'move'; share: Place; paths: string[] } | null>(null)

interface Target {
  location: Location
  items: Item[]
}
const here = computed<Target>(() => ({ location: location.value, items: chosen.value }))
const treeTarget = computed<Target | null>(() =>
  aimed.value
    ? { location: folderAt(aimed.value.share, aimed.value.path), items: [aimed.value] }
    : null,
)

function isFavourite(share: number, path: string): boolean {
  return (favourites.value ?? []).some((f) => f.share_id === share && f.path === path)
}

/** The folder the favourite action is about: one chosen, or the one shown; never on a drive. */
function favouriteFolder(target: Target): { share: number; path: string } | null {
  const one = target.items.length === 1 ? target.items[0] : undefined
  if (one) {
    return one.kind === 'directory' && one.volumeFolder?.access !== null && !isDrive(one.share)
      ? { share: one.share, path: one.path }
      : null
  }
  const at = target.location
  return at.kind === 'folder' && !isDrive(at.share) && !at.snapshot && !inBin(at.path)
    ? { share: at.share, path: at.path }
    : null
}

/** The folder on a volume the share, versions and properties actions are about. */
function volumeFolderOf(target: Target): number | null {
  const one = target.items.length === 1 ? target.items[0] : undefined
  if (one) return one.volumeFolder && !isDrive(one.share) ? one.share : null
  const at = target.location
  return at.kind === 'folder' && at.path === '' && !isDrive(at.share) && at.share !== HOME
    ? at.share
    : null
}

/** The drive the eject and mount actions are about: one in the tree, or the one shown. */
function driveIdOf(target: Target): string | null {
  const one = target.items.length === 1 ? target.items[0] : undefined
  if (one?.drive && isDrive(one.share)) return one.share
  const at = target.location
  return at.kind === 'folder' && isDrive(at.share) ? at.share : null
}

function situation(target: Target): Situation {
  const at = target.location
  const inFolder = at.kind === 'folder'
  const access = inFolder ? accessOf(at.share) : undefined
  const driveId = driveIdOf(target)
  const aimedDrive = target.items.length === 1 ? target.items[0]?.drive : undefined
  const isHere = target === here.value
  const writable =
    inFolder &&
    !at.snapshot &&
    access === 'read_write' &&
    (isHere ? (folder.value?.writable ?? false) : true)
  const own =
    inFolder && bin.value !== null && (at.path === bin.value || at.path.startsWith(`${bin.value}/`))
  const favourite = favouriteFolder(target)
  const shareFolder = volumeFolderOf(target)
  const shareTarget =
    shareFolder !== null
      ? volumes.value?.flatMap((v) => v.folders).find((f) => f.id === shareFolder)
      : undefined
  const versionsOf =
    shareFolder ?? (inFolder && !isDrive(at.share) && at.share !== HOME ? at.share : null)
  return {
    kind: at.kind,
    searching: isHere && results.value !== undefined,
    writable,
    snapshot: inFolder && at.snapshot !== null,
    ownBin: own,
    binToEmpty:
      (at.kind === 'bin' && !at.path && allItems.value.length > 0) ||
      (own && isHere && at.path === bin.value && allItems.value.length > 0),
    shareWritable: access === 'read_write',
    canManage: props.canManage === true,
    clipboard: clipboard.value !== null,
    favourite: favourite !== null && isFavourite(favourite.share, favourite.path),
    favouriteTarget: favourite !== null,
    hasSnapshots: versionsOf !== null && place(versionsOf) !== undefined,
    shareTarget: shareTarget ? { shared: shareTarget.shared } : null,
    drive: driveId === null ? null : (driveOf(driveId)?.state ?? aimedDrive?.state ?? 'unmounted'),
    items: target.items.filter((i) => !i.unreadable),
  }
}

const ICONS: Record<ActionId, MenuAction['icon']> = {
  open: FolderOpen,
  openLocation: FolderSearch,
  download: Download,
  newFolder: FolderPlus,
  upload: Upload,
  cut: Scissors,
  copy: Copy,
  paste: ClipboardPaste,
  rename: Pencil,
  delete: Trash2,
  restore: RotateCcw,
  emptyBin: Trash2,
  copyBack: History,
  favourite: Star,
  share: Network,
  previousVersions: History,
  eject: Unplug,
  mount: Plug,
  properties: Info,
}
const KEYS: Partial<Record<ActionId, string>> = {
  open: 'Enter',
  cut: 'Ctrl+X',
  copy: 'Ctrl+C',
  paste: 'Ctrl+V',
  rename: 'F2',
  delete: 'Del',
}
const GROUPS = new Set<ActionId>([
  'newFolder',
  'cut',
  'restore',
  'favourite',
  'eject',
  'properties',
])

function menu(target: Target): MenuAction[] {
  return actionsFor(situation(target)).map((state) => ({
    id: state.id,
    label: t(`explorer.actions.${state.id}${state.undo ? 'Undo' : ''}`),
    icon: ICONS[state.id],
    disabled: state.disabled,
    destructive: state.id === 'delete' || state.id === 'emptyBin',
    separated: GROUPS.has(state.id),
    shortcut: KEYS[state.id],
  }))
}
/** What may be done here now: for the keys. */
const available = computed(() => menu(here.value))
const contextMenu = computed(() => menu(treeTarget.value ?? here.value))

function fail(failure: unknown): void {
  notifications.notify('error', problemMessage(failure))
}

function go(next: Location): void {
  aimed.value = null
  treeOpen.value = false
  history.go(next)
}

function save(href: string): void {
  const link = document.createElement('a')
  link.href = href
  link.download = ''
  link.click()
}

// Dialogs.
const previewing = ref<{ share: Place; path: string; snapshot: string | null } | null>(null)
const previewOpen = computed({
  get: () => previewing.value !== null,
  set: (open) => {
    if (!open) previewing.value = null
  },
})
const naming = ref<{ mode: 'folder' | 'volumeFolder' | 'rename'; target: Target } | null>(null)
const namingError = ref<string | null>(null)
/** The name given is being carried out; a folder on a volume takes seconds. */
const namingBusy = ref(false)
const deleting = ref<Target | null>(null)
const emptying = ref<Target | null>(null)
const deletingFolder = ref<{ id: number; name: string; path: string } | null>(null)
const properties = ref<{ id: number; tab?: 'general' | 'permissions' | 'sharing' } | null>(null)
const info = ref<EntryInfo | null>(null)
const versions = ref<{ share: number; path: string } | null>(null)
const managing = ref<number | null>(null)
const managedShare = (id: number | null) => managed.data.value?.find((s) => s.id === id) ?? null

function typeOf(item: Item): string {
  if (item.drive || (isDrive(item.share) && item.path === '')) return t('explorer.types.drive')
  if (item.volumeFolder) {
    return item.volumeFolder.shared
      ? t('explorer.types.sharedFolder')
      : t('explorer.types.volumeFolder')
  }
  if (item.kind === 'directory') return t('explorer.types.folder')
  if (item.kind === 'symlink') return t('explorer.types.link')
  if (item.kind === 'other') return t('explorer.types.other')
  const ext = extension(item.name)
  return ext ? t('explorer.types.file', { ext }) : t('explorer.types.plainFile')
}

function open(item: Item): void {
  if (item.volumeFolder?.access === null) return
  if (item.drive && item.drive.state !== 'mounted') return
  const at = location.value
  const snapshot = at.kind === 'folder' ? at.snapshot : null
  const inside = at.kind === 'bin' ? withinBin(bin.value, item.path) : null
  if (item.kind === 'directory' && inside !== null)
    go({ kind: 'bin', share: item.share, path: inside })
  else if (item.kind === 'directory') go(folderAt(item.share, item.path, snapshot))
  else if (item.kind === 'file' && previewKind(item.name)) {
    previewing.value = { share: item.share, path: item.path, snapshot }
  } else save(downloadUrl(item.share, item.path, snapshot))
}

async function transfer(
  mode: 'copy' | 'move',
  share: Place,
  paths: string[],
  into: { share: Place; path: string },
) {
  const inside = paths.some(
    (p) => share === into.share && (into.path === p || into.path.startsWith(`${p}/`)),
  )
  if (inside || paths.every((p) => share === into.share && parentPath(p) === into.path)) return
  try {
    await actions.transfer(share, {
      mode,
      paths,
      ...targetOf(into.share),
      target_path: into.path,
    })
    notifications.notify('info', t(`files.started.${mode}`, { count: paths.length }, paths.length))
  } catch (failure) {
    fail(failure)
  }
}

/** Entries by the folder on a volume, or the drive, they are in. */
function byShare(list: Item[]): Map<Place, string[]> {
  const grouped = new Map<Place, string[]>()
  for (const item of list) grouped.set(item.share, [...(grouped.get(item.share) ?? []), item.path])
  return grouped
}

async function removeAll(list: Item[]): Promise<void> {
  try {
    for (const [share, paths] of byShare(list)) {
      for (let start = 0; start < paths.length; start += 1000) {
        await actions.remove(share, paths.slice(start, start + 1000))
      }
    }
    notifications.notify('info', t('files.started.delete', { count: list.length }, list.length))
    selected.value = new Set()
  } catch (failure) {
    fail(failure)
  }
}

async function restoreAll(list: Item[]): Promise<void> {
  try {
    let count = 0
    // Only the bins of folders on volumes: drives have none.
    for (const [share, paths] of byShare(list))
      if (!isDrive(share)) count += (await actions.restore(share, paths)).length
    notifications.notify('info', t('files.restored', { count }, count))
    selected.value = new Set()
  } catch (failure) {
    fail(failure)
  }
}

async function toggleFavourite(target: Target): Promise<void> {
  const folderOf = favouriteFolder(target)
  if (!folderOf) return
  const list = (favourites.value ?? []).map(({ share_id, path }) => ({ share_id, path }))
  const next = isFavourite(folderOf.share, folderOf.path)
    ? list.filter((f) => f.share_id !== folderOf.share || f.path !== folderOf.path)
    : [...list, { share_id: folderOf.share, path: folderOf.path }]
  try {
    await setFavourites(next)
  } catch (failure) {
    fail(failure)
  }
}

async function eject(id: string): Promise<void> {
  const name = folderName(id)
  try {
    await driveActions.eject(id)
    notifications.notify('success', t('explorer.drive.ejectedNow', { name }))
    if (clipboard.value?.share === id) clipboard.value = null
  } catch (failure) {
    fail(failure)
  }
}

async function mount(id: string): Promise<void> {
  try {
    await driveActions.mount(id)
    notifications.notify('success', t('explorer.drive.mountedNow', { name: folderName(id) }))
  } catch (failure) {
    fail(failure)
  }
}

async function setShared(id: number, shared: boolean): Promise<void> {
  try {
    await shareActions.update(id, { shared })
    notifications.notify(
      'success',
      t(shared ? 'explorer.sharedNow' : 'explorer.unsharedNow', { name: folderName(id) }),
    )
  } catch (failure) {
    fail(failure)
  }
}

// Uploads.
interface Sending {
  id: number
  name: string
  progress: number
  error?: string
  controller: AbortController
}
const uploads = ref<Sending[]>([])
let nextUpload = 0
const picker = ref<HTMLInputElement | null>(null)
const uploadInto = ref<{ share: Place; path: string } | null>(null)
const conflicts = ref<{
  files: File[]
  taken: Set<string>
  into: { share: Place; path: string }
} | null>(null)
const asking = ref(false)
watch(asking, (isOpen) => {
  if (!isOpen) resolveConflicts(false)
})

async function upload(files: File[], into: { share: Place; path: string }): Promise<void> {
  if (!files.length) return
  let existing: Set<string>
  try {
    const listed =
      shareRef.value === into.share && pathRef.value === into.path && folder.value
        ? folder.value
        : await listFolder(into.share, into.path)
    existing = new Set(listed.entries.map((e) => e.name))
  } catch (failure) {
    fail(failure)
    return
  }
  const taken = new Set(files.filter((f) => existing.has(f.name)).map((f) => f.name))
  if (taken.size > 0) {
    conflicts.value = { files, taken, into }
    asking.value = true
  } else for (const file of files) void send(file, false, into)
}

function resolveConflicts(replace: boolean): void {
  const pending = conflicts.value
  conflicts.value = null
  if (!pending) return
  for (const file of pending.files) {
    const isTaken = pending.taken.has(file.name)
    if (!isTaken || replace) void send(file, isTaken, pending.into)
  }
}

async function send(
  file: File,
  replace: boolean,
  into: { share: Place; path: string },
): Promise<void> {
  const id = nextUpload++
  const controller = new AbortController()
  uploads.value.push({ id, name: file.name, progress: 0, controller })
  const tracked = () => uploads.value.find((u) => u.id === id)
  const forget = () => (uploads.value = uploads.value.filter((u) => u.id !== id))
  try {
    await uploadFile(into.share, into.path, file, {
      replace,
      signal: controller.signal,
      onProgress: (fraction) => {
        const sending = tracked()
        if (sending) sending.progress = fraction
      },
    })
    forget()
    await actions.refresh()
  } catch (failure) {
    const sending = tracked()
    if (failure instanceof ApiProblem && failure.code === 'aborted') forget()
    else if (sending) sending.error = problemMessage(failure)
  }
}

function dismiss(sending: Sending): void {
  sending.controller.abort()
  uploads.value = uploads.value.filter((u) => u.id !== sending.id)
}

function picked(event: Event): void {
  const input = event.target as HTMLInputElement
  if (uploadInto.value) void upload([...(input.files ?? [])], uploadInto.value)
  input.value = ''
}

/** The folder new entries of `target` go into: the node aimed at, or the one shown. */
function container(target: Target): { share: Place; path: string } | null {
  const at = target.location
  return at.kind === 'folder' && !at.snapshot ? { share: at.share, path: at.path } : null
}

function run(id: ActionId, target: Target): void {
  const list = target.items.filter((i) => !i.unreadable)
  const one = list.length === 1 ? list[0] : undefined
  const into = container(target)
  const at = target.location
  switch (id) {
    case 'open':
      if (one) open(one)
      return
    case 'openLocation':
      if (one) {
        revealing = one.key
        go(folderAt(one.share, parentPath(one.path), at.kind === 'folder' ? at.snapshot : null))
      }
      return
    case 'download':
      if (!one) return
      if (one.kind === 'file')
        save(downloadUrl(one.share, one.path, at.kind === 'folder' ? at.snapshot : null))
      else save(archiveUrl(one.share, one.path, at.kind === 'folder' ? at.snapshot : null))
      return
    case 'newFolder':
      namingError.value = null
      naming.value = { mode: at.kind === 'volume' ? 'volumeFolder' : 'folder', target }
      return
    case 'upload':
      uploadInto.value = into
      picker.value?.click()
      return
    case 'cut':
    case 'copy':
      if (!list.length) return
      clipboard.value = {
        mode: id === 'cut' ? 'move' : 'copy',
        share: list[0]!.share,
        paths: list.map((i) => i.path),
      }
      return
    case 'paste': {
      const taken = clipboard.value
      if (!taken || !into) return
      void transfer(taken.mode, taken.share, taken.paths, into).then(() => {
        if (taken.mode === 'move') clipboard.value = null
      })
      return
    }
    case 'rename':
      namingError.value = null
      if (one) naming.value = { mode: 'rename', target }
      return
    case 'delete':
      if (one?.volumeFolder && !isDrive(one.share)) {
        deletingFolder.value = {
          id: one.share,
          name: one.name,
          path: `/volume${volumeOf(one.share)?.volume ?? '?'}/${one.name}`,
        }
      } else if (list.length) deleting.value = target
      return
    case 'restore':
      void restoreAll(list)
      return
    case 'emptyBin':
      emptying.value = target
      return
    case 'copyBack':
      if (at.kind === 'folder' && at.snapshot && !isDrive(at.share)) {
        const paths = list.map((i) => i.path)
        restoreFromSnapshot(at.share, at.snapshot, paths)
          .then(() => {
            notifications.notify(
              'info',
              t('files.restoringFromSnapshot', { count: paths.length }, paths.length),
            )
            selected.value = new Set()
          })
          .catch(fail)
      }
      return
    case 'favourite':
      void toggleFavourite(target)
      return
    case 'share': {
      const id = volumeFolderOf(target)
      const found = volumes.value?.flatMap((v) => v.folders).find((f) => f.id === id)
      if (id !== null && found) void setShared(id, !found.shared)
      return
    }
    case 'previousVersions': {
      const share =
        volumeFolderOf(target) ?? (at.kind === 'folder' && !isDrive(at.share) ? at.share : null)
      if (share !== null)
        versions.value = { share, path: at.kind === 'folder' && !one ? at.path : '' }
      return
    }
    case 'eject':
    case 'mount': {
      const drive = driveIdOf(target)
      if (drive !== null) void (id === 'eject' ? eject(drive) : mount(drive))
      return
    }
    case 'properties': {
      const folderId = volumeFolderOf(target)
      if (folderId !== null && props.canManage && managedShare(folderId)) {
        properties.value = { id: folderId }
        return
      }
      const found =
        folderId !== null
          ? volumes.value?.flatMap((v) => v.folders).find((f) => f.id === folderId)
          : undefined
      const onVolume = found ? { shared: found.shared, access: found.access ?? null } : undefined
      const subject =
        one ??
        (at.kind === 'folder'
          ? folderItem(at.share, at.path, at.path.split('/').pop() || folderName(at.share), {
              volumeFolder: onVolume,
            })
          : undefined)
      if (!subject) return
      info.value = {
        name: subject.name,
        type: typeOf(subject),
        location: subject.path
          ? `${folderName(subject.share)}/${subject.path}`
          : folderName(subject.share),
        size: subject.size,
        modified: subject.modified,
        ...(onVolume ? onVolume : {}),
      }
    }
  }
}

/** A right click on an entry not chosen yet chooses it alone. */
function aimEntry(item: Item): void {
  if (!selected.value.has(item.key)) selected.value = new Set([item.key])
  anchor = item.key
}

function choose(id: string, target: Target): void {
  run(id as ActionId, target)
  aimed.value = null
}

async function named(name: string): Promise<void> {
  const asked = naming.value
  if (!asked) return
  namingError.value = null
  namingBusy.value = true
  const at = asked.target.location
  try {
    if (asked.mode === 'volumeFolder' && at.kind === 'volume') {
      const volume = volumes.value?.find((v) => v.volume === at.volume)
      await shareActions.create({
        name,
        description: null,
        pool: volume?.pool ?? '',
        shared: false,
        recycle_bin: true,
        access:
          props.accountId === undefined
            ? []
            : [{ kind: 'account', id: props.accountId, access: 'read_write' }],
      })
      await client.invalidateQueries({ queryKey: ['files'] })
    } else if (asked.mode === 'folder') {
      const into = container(asked.target)
      if (into) await actions.createFolder(into.share, joinPath(into.path, name))
    } else {
      const one = asked.target.items[0]
      if (one) await actions.rename(one.share, one.path, name)
      selected.value = new Set()
    }
    naming.value = null
  } catch (failure) {
    namingError.value = problemMessage(failure)
  } finally {
    namingBusy.value = false
  }
}

const deleteForGood = computed(() => {
  const target = deleting.value
  if (!target) return false
  if (target.location.kind === 'bin') return true
  return target.items.some((i) => inBin(i.path) || !place(i.share)?.recycle_bin)
})

function emptyBin(): void {
  const target = emptying.value
  if (!target) return
  const at = target.location
  const list = at.kind === 'bin' ? allItems.value : allItems.value.filter((i) => !i.unreadable)
  void removeAll(list.filter((i) => !i.unreadable))
}

// Dragging entries onto folders, and files from the computer.
const ENTRIES = 'application/x-zendiloos-entries'
const dragging = ref(false)

function dragStart(item: Item, event: DragEvent): void {
  const list = selected.value.has(item.key) ? chosen.value : [item]
  event.dataTransfer?.setData(
    ENTRIES,
    JSON.stringify({ share: item.share, paths: list.map((i) => i.path) }),
  )
  if (event.dataTransfer) event.dataTransfer.effectAllowed = 'copyMove'
}

function dropOn(event: DragEvent, share: Place, path: string): void {
  dragging.value = false
  const files = [...(event.dataTransfer?.files ?? [])]
  if (files.length) {
    void upload(files, { share, path })
    return
  }
  const raw = event.dataTransfer?.getData(ENTRIES)
  if (!raw) return
  const dragged = JSON.parse(raw) as { share: Place; paths: string[] }
  void transfer(event.ctrlKey || event.altKey ? 'copy' : 'move', dragged.share, dragged.paths, {
    share,
    path,
  })
}

function dropHere(event: DragEvent): void {
  const into = container(here.value)
  if (into && situation(here.value).writable) dropOn(event, into.share, into.path)
  else dragging.value = false
}

// Keys where the entries have the focus.
function keydown(event: KeyboardEvent): void {
  const key = shortcut(event)
  if (key === null) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      const usable = items.value.filter((i) => !i.unreadable)
      const at = usable.findIndex((i) => i.key === anchor)
      const next =
        usable[Math.max(0, Math.min(usable.length - 1, at + (event.key === 'ArrowDown' ? 1 : -1)))]
      if (next) {
        selected.value = new Set([next.key])
        anchor = next.key
        event.preventDefault()
      }
    }
    return
  }
  event.preventDefault()
  if (key === 'selectAll') toggleAll()
  else if (key === 'up') up()
  else if (key === 'back') history.back()
  else if (key === 'forward') history.forward()
  else {
    const state = available.value.find((a) => a.id === key)
    if (state && !state.disabled) run(key, here.value)
  }
}

function up(): void {
  const at = location.value
  if (at.kind === 'bin') {
    if (at.share === undefined) return
    go(at.path ? { kind: 'bin', share: at.share, path: parentPath(at.path) } : { kind: 'bin' })
    return
  }
  if (at.kind !== 'folder') return
  if (at.path) go(folderAt(at.share, parentPath(at.path), at.snapshot))
  else {
    const volume = volumeOf(at.share)
    if (volume && at.share !== HOME) go({ kind: 'volume', volume: volume.volume })
  }
}

const canUp = computed(() => {
  const at = location.value
  if (at.kind === 'bin') return at.share !== undefined
  return (
    at.kind === 'folder' &&
    (at.path !== '' || (at.share !== HOME && volumeOf(at.share) !== undefined))
  )
})

const crumbs = computed<{ label: string; location: Location }[]>(() => {
  const at = location.value
  if (at.kind === 'bin') {
    const crumbsOfBin: { label: string; location: Location }[] = [
      { label: t('files.recycleBin'), location: { kind: 'bin' } },
    ]
    const share = at.share
    if (share === undefined) return crumbsOfBin
    crumbsOfBin.push({ label: folderName(share), location: { kind: 'bin', share, path: '' } })
    const inside = at.path ? at.path.split('/') : []
    inside.forEach((name, index) => {
      crumbsOfBin.push({
        label: name,
        location: { kind: 'bin', share, path: inside.slice(0, index + 1).join('/') },
      })
    })
    return crumbsOfBin
  }
  if (at.kind === 'volume') {
    const volume = volumes.value?.find((v) => v.volume === at.volume)
    return [
      { label: t('explorer.volume', { n: at.volume, pool: volume?.pool ?? '' }), location: at },
    ]
  }
  const list: { label: string; location: Location }[] = []
  const volume = at.share !== HOME ? volumeOf(at.share) : undefined
  if (volume) {
    list.push({
      label: t('explorer.volume', { n: volume.volume, pool: volume.pool }),
      location: { kind: 'volume', volume: volume.volume },
    })
  }
  list.push({ label: folderName(at.share), location: folderAt(at.share, '', at.snapshot) })
  const names = at.path ? at.path.split('/') : []
  names.forEach((name, index) => {
    list.push({
      label: name === RECYCLE_BIN ? t('files.recycleBin') : name,
      location: folderAt(at.share, names.slice(0, index + 1).join('/'), at.snapshot),
    })
  })
  return list
})

const snapshotTaken = computed(() => {
  const at = location.value
  const found =
    at.kind === 'folder' ? snapshots.value?.find((s) => s.label === at.snapshot) : undefined
  return found ? formatDateTime(found.taken_at * 1000) : ''
})

const binEmpty = computed(
  () =>
    ((location.value.kind === 'folder' && inBin(location.value.path)) || binShare.value !== null) &&
    folderError.value instanceof ApiProblem &&
    folderError.value.status === 404,
)
const viewError = computed(() => {
  if (location.value.kind === 'folder' || binShare.value !== null) {
    return binEmpty.value ? null : folderError.value
  }
  if (location.value.kind === 'bin') return bins.value.find((q) => q.error)?.error ?? null
  return volumesError.value
})

// When a copy, move, delete or share change is over, what is shown may differ.
watch(
  () =>
    jobs.list.filter(
      (job) =>
        (job.kind.startsWith('files.') || job.kind.startsWith('sharing.')) &&
        job.state !== 'running',
    ).length,
  () => {
    void actions.refresh()
    void client.invalidateQueries({ queryKey: ['files', 'volumes'] })
  },
)

const treeOpen = ref(false)
provide(EXPLORER, {
  current: location,
  go,
  aim: (item: Item) => (aimed.value = item),
  drop: (event: DragEvent, share: Place, path: string) => dropOn(event, share, path),
})
</script>

<!-- The NAS's files: a tree of places on the left, what is in one on the right. -->
<template>
  <div class="relative flex h-full min-h-0">
    <ContextMenu :actions="contextMenu" @select="(id) => choose(id, treeTarget ?? here)">
      <aside
        :class="
          cn(
            'w-64 shrink-0 overflow-y-auto border-r bg-muted/40',
            treeOpen
              ? 'absolute inset-y-0 left-0 z-30 bg-background shadow-lg'
              : 'hidden @3xl:block',
          )
        "
        @contextmenu.capture="aimed = null"
      >
        <NavigationTree
          :home="hasHome"
          :favourites="favourites ?? []"
          :volumes="volumes ?? []"
          :drives="canManageStorage ? (drives ?? []) : []"
        />
      </aside>
    </ContextMenu>
    <div
      v-if="treeOpen"
      class="absolute inset-0 z-20 bg-black/20 @3xl:hidden"
      @click="treeOpen = false"
    />

    <div class="flex min-h-0 min-w-0 flex-1 flex-col">
      <div class="flex items-center gap-1 border-b px-2 py-1.5">
        <Button
          variant="ghost"
          size="icon"
          class="@3xl:hidden"
          :title="t('explorer.navigation')"
          :aria-label="t('explorer.navigation')"
          @click="treeOpen = !treeOpen"
        >
          <PanelLeft />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          :disabled="!history.canBack.value"
          :title="t('explorer.back')"
          :aria-label="t('explorer.back')"
          @click="history.back()"
        >
          <ArrowLeft />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          :disabled="!history.canForward.value"
          :title="t('explorer.forward')"
          :aria-label="t('explorer.forward')"
          @click="history.forward()"
        >
          <ArrowRight />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          :disabled="!canUp"
          :title="t('files.up')"
          :aria-label="t('files.up')"
          @click="up"
        >
          <ArrowUp />
        </Button>
        <ol
          class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto rounded-md border bg-background px-2 py-1 text-sm"
          :aria-label="t('files.location')"
        >
          <li
            v-for="(crumb, index) in crumbs"
            :key="index"
            class="flex min-w-0 shrink-0 items-center gap-1"
          >
            <span v-if="index" class="text-muted-foreground">›</span>
            <button
              type="button"
              class="truncate rounded px-1 hover:bg-accent"
              @click="go(crumb.location)"
            >
              {{ crumb.label }}
            </button>
          </li>
        </ol>
        <label class="relative hidden @xl:block">
          <Search
            class="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-muted-foreground"
          />
          <input
            v-model="search"
            type="search"
            class="h-8 w-44 rounded-md border bg-background pr-2 pl-8 text-sm"
            :placeholder="t('explorer.search.placeholder')"
            :aria-label="t('explorer.search.placeholder')"
            @keydown.escape="search = ''"
          />
        </label>
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
      </div>

      <div
        v-if="location.kind === 'folder' && location.snapshot !== null"
        class="flex items-center gap-2 border-b bg-amber-500/10 px-3 py-2 text-sm"
        role="status"
      >
        <History class="size-4 shrink-0" />
        <span class="flex-1">{{ t('files.inSnapshot', { time: snapshotTaken }) }}</span>
        <Button size="sm" variant="outline" @click="go(folderAt(location.share, location.path))">{{
          t('files.backToNow')
        }}</Button>
      </div>
      <div
        v-if="location.kind === 'folder' && searchText"
        class="flex items-center gap-2 border-b px-3 py-1.5 text-xs text-muted-foreground"
        role="status"
      >
        <FolderSearch class="size-4 shrink-0" />
        <span class="flex-1">{{ searchStatus }}</span>
        <Button size="sm" variant="ghost" class="h-7" @click="search = ''">{{
          t('explorer.search.clear')
        }}</Button>
      </div>
      <div
        v-if="clipboard"
        class="flex items-center gap-2 border-b px-3 py-1.5 text-xs text-muted-foreground"
      >
        <span class="flex-1">{{
          t(
            `explorer.clipboard.${clipboard.mode}`,
            { count: clipboard.paths.length },
            clipboard.paths.length,
          )
        }}</span>
        <Button
          size="icon"
          variant="ghost"
          class="size-7"
          :title="t('files.clearClipboard')"
          :aria-label="t('files.clearClipboard')"
          @click="clipboard = null"
        >
          <X />
        </Button>
      </div>

      <ContextMenu :actions="contextMenu" @select="(id) => choose(id, treeTarget ?? here)">
        <div
          class="relative min-h-0 flex-1 overflow-auto outline-none"
          tabindex="0"
          :aria-busy="isFetching || searchBusy"
          :aria-label="t('explorer.entries')"
          @keydown="keydown"
          @contextmenu.capture="aimed = null"
          @click.self="selected = new Set()"
          @dragover.prevent="dragging = location.kind === 'folder'"
          @dragleave.self="dragging = false"
          @drop.prevent="dropHere"
        >
          <Alert v-if="placesError" variant="destructive" class="m-4">{{
            problemMessage(placesError)
          }}</Alert>
          <p
            v-else-if="
              !volumesPending &&
              !hasHome &&
              !(volumes ?? []).some((v) => v.folders.length) &&
              !canManage
            "
            class="p-6 text-sm text-muted-foreground"
          >
            {{ t('files.noShares') }}
          </p>
          <p v-else-if="binEmpty" class="p-6 text-sm text-muted-foreground">
            {{ t('files.binEmpty') }}
          </p>
          <Alert v-else-if="viewError" variant="destructive" class="m-4">{{
            problemMessage(viewError)
          }}</Alert>
          <template v-else>
            <DetailsView
              :items="items"
              :selected="selected"
              :sort="sort"
              :type-of="typeOf"
              :show-origin="location.kind === 'bin' || results !== undefined"
              :origin-label="
                results !== undefined ? t('explorer.columns.folder') : t('explorer.columns.origin')
              "
              @sort="
                (key) => (sort = { key, descending: sort.key === key ? !sort.descending : false })
              "
              @pick="pick"
              @toggle="toggle"
              @toggle-all="toggleAll"
              @open="open"
              @aim="aimEntry"
              @drag-start="dragStart"
              @drop="(item, event) => dropOn(event, item.share, item.path)"
            />
            <p v-if="!items.length" class="p-6 text-sm text-muted-foreground">
              {{
                results !== undefined
                  ? t('explorer.search.nothing')
                  : location.kind === 'bin' || (location.kind === 'folder' && inBin(location.path))
                    ? t('explorer.binEmpty')
                    : location.kind === 'volume'
                      ? t('explorer.volumeEmpty')
                      : t('files.empty')
              }}
            </p>
            <p
              v-if="folder?.truncated && location.kind === 'folder'"
              class="p-2 text-xs text-muted-foreground"
            >
              {{ t('files.truncated', { count: folder.entries.length }) }}
            </p>
          </template>
          <div
            v-if="dragging"
            class="pointer-events-none absolute inset-2 grid place-items-center rounded-xl border-2 border-dashed border-primary bg-primary/5 text-sm font-medium text-primary"
          >
            {{ t('files.dropHere') }}
          </div>
        </div>
      </ContextMenu>

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
    </div>

    <input
      ref="picker"
      type="file"
      multiple
      class="hidden"
      :aria-label="t('files.upload')"
      @change="picked"
    />
    <FilePreview
      v-model:open="previewOpen"
      :share="previewing?.share ?? 0"
      :path="previewing?.path ?? null"
      :snapshot="previewing?.snapshot ?? null"
    />
    <NameDialog
      :open="naming !== null"
      :title="
        naming?.mode === 'rename'
          ? t('files.renameTitle', { name: naming.target.items[0]?.name })
          : t('files.newFolder')
      "
      :initial="naming?.mode === 'rename' ? (naming.target.items[0]?.name ?? '') : ''"
      :submit-label="naming?.mode === 'rename' ? t('files.rename') : t('files.create')"
      :hint="naming?.mode === 'volumeFolder' ? t('explorer.volumeFolderHint') : undefined"
      :error="namingError"
      :busy="namingBusy"
      @update:open="(value) => !value && !namingBusy && (naming = null)"
      @submit="named"
    />
    <ConfirmDialog
      :open="deleting !== null"
      :title="deleteForGood ? t('files.deleteForGoodTitle') : t('files.deleteTitle')"
      :description="
        deleteForGood
          ? t(
              'files.deleteForGood',
              { count: deleting?.items.length ?? 0 },
              deleting?.items.length ?? 0,
            )
          : t(
              'files.deleteToBin',
              { count: deleting?.items.length ?? 0 },
              deleting?.items.length ?? 0,
            )
      "
      :confirm-label="t('files.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(value) => !value && (deleting = null)"
      @confirm="deleting && removeAll(deleting.items)"
    />
    <ConfirmDialog
      :open="emptying !== null"
      :title="t('files.emptyBinTitle')"
      :description="t('explorer.emptyBinDescription')"
      :confirm-label="t('files.emptyBin')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(value) => !value && (emptying = null)"
      @confirm="emptyBin"
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
    <DeleteFolderDialog
      :open="deletingFolder !== null"
      :folder="deletingFolder"
      @update:open="(value) => !value && (deletingFolder = null)"
    />
    <FolderPropertiesDialog
      :open="properties !== null"
      :share="managedShare(properties?.id ?? null)"
      :tab="properties?.tab"
      @update:open="(value) => !value && (properties = null)"
    />
    <EntryPropertiesDialog
      :open="info !== null"
      :entry="info"
      @update:open="(value) => !value && (info = null)"
    />
    <PreviousVersionsDialog
      :open="versions !== null"
      :share="versions?.share ?? null"
      :name="folderName(versions?.share ?? HOME)"
      :can-manage="canManage === true"
      @update:open="(value) => !value && (versions = null)"
      @browse="(label) => versions && go(folderAt(versions.share, versions.path, label))"
      @manage="managing = versions?.share ?? null"
    />
    <SnapshotsDialog
      :open="managing !== null"
      :share="managedShare(managing)"
      @update:open="(value) => !value && (managing = null)"
    />
  </div>
</template>
