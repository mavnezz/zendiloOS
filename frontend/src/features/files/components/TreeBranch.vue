<script setup lang="ts">
import { ChevronRight, Folder, FolderLock, House, Network } from 'lucide-vue-next'
import { computed, inject, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'

import { useFolder } from '../api'
import type { Item } from '../lib/actions'
import { folderAt, joinPath } from '../lib/location'
import { EXPLORER, folderItem } from '../lib/tree'

/** A folder in the tree, its folders below it once opened. */
const props = defineProps<{
  share: number
  path: string
  name: string
  depth: number
  /** The personal folder: its own icon. */
  home?: boolean
  /** A folder directly on a volume. */
  volumeFolder?: Item['volumeFolder']
}>()

const { t } = useI18n()
const explorer = inject(EXPLORER)!
const expanded = ref(false)
const locked = computed(() => props.volumeFolder?.access === null)
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
const icon = computed(() =>
  props.home ? House : locked.value ? FolderLock : props.volumeFolder?.shared ? Network : Folder,
)
const item = computed(() =>
  folderItem(props.share, props.path, props.name, { volumeFolder: props.volumeFolder }),
)
</script>

<template>
  <li role="treeitem" :aria-expanded="locked ? undefined : expanded" :aria-selected="here">
    <div
      :class="
        cn(
          'flex items-center gap-1 rounded-md py-1 pr-2 text-sm',
          here ? 'bg-primary/10 font-medium text-primary' : 'hover:bg-foreground/5',
          locked && 'text-muted-foreground',
        )
      "
      :style="{ paddingLeft: `${depth * 12 + 4}px` }"
      @contextmenu="explorer.aim(item)"
      @dragover.prevent
      @drop.prevent.stop="explorer.drop($event, share, path)"
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
        :title="locked ? t('explorer.noAccess') : undefined"
        @click="explorer.go(folderAt(share, path))"
      >
        <component
          :is="icon"
          :class="cn('size-4 shrink-0', home ? 'text-sky-500' : 'fill-amber-400/30 text-amber-500')"
        />
        <span class="truncate">{{ name }}</span>
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
