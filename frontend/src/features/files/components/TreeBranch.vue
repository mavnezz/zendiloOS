<script setup lang="ts">
import { ChevronRight, Folder, FolderLock, House, Network, Usb } from 'lucide-vue-next'
import { computed, inject, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'

import { useFolder } from '../api'
import type { Item } from '../lib/actions'
import { folderAt, joinPath, type Place } from '../lib/location'
import { EXPLORER, folderItem } from '../lib/tree'

/** A folder in the tree, its folders below it once opened. */
const props = defineProps<{
  share: Place
  path: string
  name: string
  depth: number
  /** The personal folder: its own icon. */
  home?: boolean
  /** A folder directly on a volume. */
  volumeFolder?: Item['volumeFolder']
  /** A removable drive itself: opened only while mounted. */
  drive?: Item['drive']
}>()

const { t } = useI18n()
const explorer = inject(EXPLORER)!
const expanded = ref(false)
/** Something is dragged over this folder: it would land here. */
const over = ref(false)
const unmounted = computed(() => props.drive !== undefined && props.drive.state !== 'mounted')
const locked = computed(() => props.volumeFolder?.access === null || unmounted.value)
const lockedTitle = computed(() => {
  if (!locked.value) return undefined
  if (!props.drive) return t('explorer.noAccess')
  return props.drive.problem ?? t(`explorer.drive.${props.drive.state}`)
})
const { data: folder } = useFolder(
  computed(() => (expanded.value && !locked.value ? props.share : null)),
  computed(() => props.path),
)
const children = computed(() =>
  (folder.value?.entries ?? [])
    .filter(
      (entry) => entry.kind === 'directory' && !entry.unreadable && !entry.name.startsWith('.'),
    )
    .map((entry) => entry.name)
    .sort((a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' })),
)
const here = computed(() => {
  const current = explorer.current.value
  return current.kind === 'folder' && current.share === props.share && current.path === props.path
})
const icon = computed(() => {
  if (props.home) return House
  if (props.drive) return Usb
  if (locked.value) return FolderLock
  return props.volumeFolder?.shared ? Network : Folder
})
const item = computed(() =>
  folderItem(props.share, props.path, props.name, {
    volumeFolder: props.volumeFolder,
    drive: props.drive,
  }),
)

function dropped(event: DragEvent): void {
  over.value = false
  if (!locked.value) explorer.drop(event, props.share, props.path)
}
</script>

<template>
  <li role="treeitem" :aria-expanded="locked ? undefined : expanded" :aria-selected="here">
    <div
      :class="
        cn(
          'flex items-center gap-1 rounded-md py-1 pr-2 text-sm',
          here ? 'bg-primary/10 font-medium text-primary' : 'hover:bg-foreground/5',
          locked && 'text-muted-foreground',
          over && 'bg-primary/10 ring-2 ring-primary ring-inset',
        )
      "
      :style="{ paddingLeft: `${depth * 12 + 4}px` }"
      :data-over="over || undefined"
      @contextmenu="explorer.aim(item)"
      @dragover.prevent="over = !locked"
      @dragleave="over = false"
      @drop.prevent.stop="dropped"
    >
      <button
        type="button"
        class="grid size-5 shrink-0 place-items-center rounded text-muted-foreground hover:bg-foreground/10 disabled:invisible"
        :disabled="locked"
        :aria-label="expanded ? t('explorer.collapse', { name }) : t('explorer.expand', { name })"
        @click="expanded = !expanded"
      >
        <ChevronRight :class="cn('size-3.5 transition', expanded && 'rotate-90')" />
      </button>
      <button
        type="button"
        class="flex min-w-0 flex-1 items-center gap-1.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
        :disabled="locked"
        :title="lockedTitle"
        @click="explorer.go(folderAt(share, path))"
      >
        <component
          :is="icon"
          :class="
            cn(
              'size-4 shrink-0',
              home || drive ? 'text-sky-500' : 'fill-amber-400/30 text-amber-500',
            )
          "
        />
        <span class="truncate">{{ name }}</span>
        <span v-if="unmounted" class="ml-auto shrink-0 text-xs">{{
          t(`explorer.drive.${drive?.state}`)
        }}</span>
      </button>
    </div>
    <ul v-if="expanded && children.length" role="group">
      <TreeBranch
        v-for="child in children"
        :key="child"
        :share="share"
        :path="joinPath(path, child)"
        :name="child"
        :depth="depth + 1"
      />
    </ul>
  </li>
</template>
