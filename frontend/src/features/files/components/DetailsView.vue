<script setup lang="ts">
import {
  ArrowDown,
  ArrowUp,
  File,
  Folder,
  FolderLock,
  Link,
  Network,
  TriangleAlert,
} from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { formatBytes, formatDateTime } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'

import { type Item, type SortKey } from '../lib/actions'

/** The entries where the explorer is, as rows with their details. */
const props = defineProps<{
  items: Item[]
  selected: Set<string>
  sort: { key: SortKey; descending: boolean }
  typeOf: (item: Item) => string
  /** Where each entry was deleted from (the recycle bin view), or is (a search's findings). */
  showOrigin: boolean
  /** The heading of that column. */
  originLabel?: string
}>()
const emit = defineEmits<{
  sort: [key: SortKey]
  /** A click, with Ctrl/Shift as pressed. */
  pick: [item: Item, event: MouseEvent]
  toggle: [item: Item]
  toggleAll: []
  open: [item: Item]
  aim: [item: Item]
  dragStart: [item: Item, event: DragEvent]
  drop: [item: Item, event: DragEvent]
}>()

const { t } = useI18n()

function icon(item: Item) {
  if (item.volumeFolder?.access === null) return FolderLock
  if (item.volumeFolder?.shared) return Network
  return item.kind === 'directory' ? Folder : item.kind === 'symlink' ? Link : File
}

const usable = () => props.items.filter((item) => !item.unreadable)
const allSelected = () =>
  usable().length > 0 && usable().every((item) => props.selected.has(item.key))

const COLUMNS: SortKey[] = ['name', 'modified', 'type', 'size']
const hidden: Record<SortKey, string> = {
  name: '',
  modified: 'hidden @2xl:table-cell',
  type: 'hidden @xl:table-cell',
  size: 'hidden @lg:table-cell',
}

/** A tap opens on a touch screen, where there is no double click. */
function pointerUp(item: Item, event: PointerEvent): void {
  if (event.pointerType === 'touch' && !item.unreadable) emit('open', item)
}
</script>

<template>
  <table class="w-full text-sm" :aria-multiselectable="true">
    <thead class="sticky top-0 z-10 bg-background text-left text-xs text-muted-foreground">
      <tr class="border-b">
        <th class="w-8 p-2">
          <input
            type="checkbox"
            class="size-4 accent-primary"
            :checked="allSelected()"
            :aria-label="t('files.selectAll')"
            @change="emit('toggleAll')"
          />
        </th>
        <th
          v-for="column in COLUMNS"
          :key="column"
          :class="cn('p-0 font-medium', hidden[column], column === 'size' && 'text-right')"
          :aria-sort="
            sort.key === column ? (sort.descending ? 'descending' : 'ascending') : undefined
          "
        >
          <button
            type="button"
            :class="
              cn(
                'flex w-full items-center gap-1 p-2 hover:text-foreground',
                column === 'size' && 'justify-end',
              )
            "
            @click="emit('sort', column)"
          >
            {{ t(`explorer.columns.${column}`) }}
            <template v-if="sort.key === column">
              <ArrowDown v-if="sort.descending" class="size-3" />
              <ArrowUp v-else class="size-3" />
            </template>
          </button>
        </th>
        <th v-if="showOrigin" class="hidden p-2 font-medium @xl:table-cell">
          {{ originLabel ?? t('explorer.columns.origin') }}
        </th>
      </tr>
    </thead>
    <tbody>
      <tr
        v-for="(item, index) in items"
        :key="item.unreadable ? `unreadable:${index}` : item.key"
        :class="
          cn(
            'border-b select-none last:border-b-0',
            selected.has(item.key) ? 'bg-primary/10' : 'hover:bg-accent/50',
            item.volumeFolder?.access === null && 'text-muted-foreground',
          )
        "
        :aria-selected="selected.has(item.key)"
        :draggable="!item.unreadable && !item.volumeFolder"
        @click="!item.unreadable && emit('pick', item, $event)"
        @dblclick="!item.unreadable && emit('open', item)"
        @contextmenu="!item.unreadable && emit('aim', item)"
        @dragstart="emit('dragStart', item, $event)"
        @dragover="item.kind === 'directory' && $event.preventDefault()"
        @drop.prevent.stop="item.kind === 'directory' && emit('drop', item, $event)"
      >
        <td class="p-2" @click.stop>
          <input
            v-if="!item.unreadable"
            type="checkbox"
            class="size-4 accent-primary"
            :checked="selected.has(item.key)"
            :aria-label="item.name"
            @change="emit('toggle', item)"
          />
        </td>
        <td class="w-full max-w-0 p-2">
          <span
            v-if="item.unreadable"
            class="flex min-w-0 items-center gap-2 text-muted-foreground"
            :title="t('files.unreadableName')"
          >
            <TriangleAlert
              class="size-4 shrink-0 text-amber-600 dark:text-amber-400"
              aria-hidden="true"
            />
            <span class="truncate">{{ item.name }}</span>
            <span class="sr-only">{{ t('files.unreadableName') }}</span>
          </span>
          <button
            v-else
            type="button"
            class="flex w-full min-w-0 items-center gap-2 text-left outline-none focus-visible:underline"
            @pointerup="pointerUp(item, $event)"
            @keydown.enter.stop.prevent="emit('open', item)"
          >
            <component
              :is="icon(item)"
              :class="
                cn(
                  'size-4 shrink-0',
                  item.kind === 'directory'
                    ? 'fill-amber-400/30 text-amber-500'
                    : 'text-muted-foreground',
                )
              "
            />
            <span class="truncate">{{ item.name }}</span>
          </button>
        </td>
        <td class="hidden p-2 whitespace-nowrap text-muted-foreground @2xl:table-cell">
          {{ item.modified !== null ? formatDateTime(item.modified * 1000) : '' }}
        </td>
        <td class="hidden truncate p-2 text-muted-foreground @xl:table-cell">
          {{ typeOf(item) }}
        </td>
        <td class="hidden p-2 text-right whitespace-nowrap tabular-nums @lg:table-cell">
          {{ item.kind === 'file' && item.size !== null ? formatBytes(item.size) : '' }}
        </td>
        <td v-if="showOrigin" class="hidden truncate p-2 text-muted-foreground @xl:table-cell">
          {{ item.origin }}
        </td>
      </tr>
    </tbody>
  </table>
</template>
