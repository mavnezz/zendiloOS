<script setup lang="ts">
import { TriangleAlert } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'

import type { Item } from '../lib/actions'
import { fileIcon } from '../lib/fileIcon'

/**
 * The entries where the explorer is, as tiles: a large icon, the name
 * below it. Chosen, opened, dragged and dropped on as the rows of the list.
 */
defineProps<{
  items: Item[]
  selected: Set<string>
  /** Where each entry was deleted from (the recycle bin view), or is (a search's findings). */
  showOrigin: boolean
}>()
const emit = defineEmits<{
  /** A click, with Ctrl/Shift as pressed. */
  pick: [item: Item, event: MouseEvent]
  toggle: [item: Item]
  open: [item: Item]
  aim: [item: Item]
  dragStart: [item: Item, event: DragEvent]
  drop: [item: Item, event: DragEvent]
  /** A click between the tiles: nothing is chosen any more. */
  clear: []
}>()

const { t } = useI18n()

/** A tap opens on a touch screen, where there is no double click. */
function pointerUp(item: Item, event: PointerEvent): void {
  if (event.pointerType === 'touch' && !item.unreadable) emit('open', item)
}

/** Entries that cannot be opened (another's personal folder, a folder without access) look quieter. */
const quiet = (item: Item) => item.volumeFolder?.access === null || item.personal?.own === false
/** Folders take what is dropped on them; the personal folders do not. */
const target = (item: Item) => item.kind === 'directory' && !item.personal
</script>

<template>
  <ul
    class="grid grid-cols-[repeat(auto-fill,minmax(7rem,1fr))] content-start gap-1 p-2"
    data-view="grid"
    @click.self="emit('clear')"
  >
    <li
      v-for="(item, index) in items"
      :key="item.unreadable ? `unreadable:${index}` : item.key"
      :class="
        cn(
          'group relative flex flex-col items-center rounded-lg border border-transparent px-1.5 pt-3 pb-2 text-center select-none',
          selected.has(item.key) ? 'border-primary/30 bg-primary/10' : 'hover:bg-muted/60',
          (quiet(item) || item.unreadable) && 'text-muted-foreground',
        )
      "
      :data-selected="selected.has(item.key) || undefined"
      :draggable="!item.unreadable && !item.volumeFolder && !item.personal"
      @click="!item.unreadable && emit('pick', item, $event)"
      @dblclick="!item.unreadable && emit('open', item)"
      @contextmenu="!item.unreadable && emit('aim', item)"
      @dragstart="emit('dragStart', item, $event)"
      @dragover="target(item) && $event.preventDefault()"
      @drop.prevent.stop="target(item) && emit('drop', item, $event)"
    >
      <input
        v-if="!item.unreadable"
        type="checkbox"
        :class="
          cn(
            'absolute top-1.5 left-1.5 size-4 accent-primary opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100',
            selected.has(item.key) && 'opacity-100',
          )
        "
        :checked="selected.has(item.key)"
        :aria-label="item.name"
        @click.stop
        @dblclick.stop
        @change="emit('toggle', item)"
      />
      <div
        v-if="item.unreadable"
        class="flex w-full flex-col items-center gap-1.5"
        :title="t('files.unreadableName')"
      >
        <TriangleAlert
          class="size-12 shrink-0 text-amber-600 dark:text-amber-400"
          :stroke-width="1.25"
          aria-hidden="true"
        />
        <span class="line-clamp-2 w-full text-xs wrap-anywhere" data-name>{{ item.name }}</span>
        <span class="sr-only">{{ t('files.unreadableName') }}</span>
      </div>
      <button
        v-else
        type="button"
        class="flex w-full flex-col items-center gap-1.5 rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring"
        :title="item.origin ? `${item.name}\n${item.origin}` : item.name"
        @pointerup="pointerUp(item, $event)"
        @keydown.enter.stop.prevent="emit('open', item)"
      >
        <component
          :is="fileIcon(item).icon"
          :class="cn('size-12 shrink-0', fileIcon(item).tone)"
          :stroke-width="1.25"
          aria-hidden="true"
        />
        <span class="line-clamp-2 w-full text-xs leading-snug wrap-break-word" data-name>{{
          item.name
        }}</span>
        <span
          v-if="showOrigin && item.origin"
          class="w-full truncate text-2xs text-muted-foreground"
          >{{ item.origin }}</span
        >
      </button>
    </li>
  </ul>
</template>
