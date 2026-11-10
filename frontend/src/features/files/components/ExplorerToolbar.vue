<script setup lang="ts">
import { ArrowDown, ArrowDownUp, ArrowUp, Ellipsis, LayoutGrid, List } from 'lucide-vue-next'
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from 'reka-ui'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'
import { Button } from '@/shared/ui/button'
import type { MenuAction } from '@/shared/ui/context-menu'

import type { SortKey } from '../lib/actions'
import type { ExplorerView } from '../lib/preferences'

/**
 * What can be done where the explorer is, as buttons: the actions used most
 * on their own, the rest behind "More"; on the right, how the entries are
 * shown. `actions` are those the context menu offers for what is chosen,
 * so both always agree on what is offered and what is greyed out.
 */
const props = defineProps<{
  actions: MenuAction[]
  view: ExplorerView
  sort: { key: SortKey; descending: boolean }
}>()
const emit = defineEmits<{
  select: [id: string]
  'update:view': [view: ExplorerView]
  sort: [key: SortKey]
}>()

const { t } = useI18n()

/** Shown as buttons, where they are offered. */
const PRIMARY = ['upload', 'newFolder', 'download', 'delete']
/** In a recycle bin, getting entries back and emptying it come first. */
const BIN_PRIMARY = ['restore', 'emptyBin']
const SORT_KEYS: SortKey[] = ['name', 'modified', 'type', 'size']

const primaryIds = computed(() =>
  props.actions.some((a) => a.id === 'restore' || a.id === 'emptyBin') ? BIN_PRIMARY : PRIMARY,
)
const primary = computed(() =>
  primaryIds.value.flatMap((id) => props.actions.filter((a) => a.id === id)),
)
const more = computed(() => props.actions.filter((a) => !primaryIds.value.includes(a.id)))

/** What a button says when hovered: its name, and its key if it has one. */
const hint = (action: MenuAction) =>
  action.shortcut ? `${action.label} (${action.shortcut})` : action.label

const sortTitle = computed(() =>
  t(props.sort.descending ? 'explorer.sort.descending' : 'explorer.sort.ascending', {
    column: t(`explorer.columns.${props.sort.key}`),
  }),
)

/** Icon only in a narrow window, with its name beside it in a wide one. */
const compact = '@4xl:w-auto @4xl:px-3'
const item =
  'flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-sm outline-none select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-[highlighted]:bg-accent'
const menu = 'z-[1100] min-w-52 rounded-xl border bg-popover p-1 text-popover-foreground shadow-lg'
const toggle = (active: boolean) =>
  cn(
    'grid size-7 place-items-center rounded outline-none focus-visible:ring-2 focus-visible:ring-ring',
    active
      ? 'bg-muted text-foreground shadow-xs dark:bg-foreground/15'
      : 'text-muted-foreground hover:text-foreground',
  )
</script>

<template>
  <div class="flex items-center gap-1.5 px-2 pb-2">
    <div
      class="flex min-w-0 flex-1 items-center gap-1.5"
      role="group"
      :aria-label="t('explorer.toolbar.label')"
    >
      <Button
        v-for="action in primary"
        :key="action.id"
        variant="outline"
        size="icon"
        :class="
          cn(
            compact,
            action.destructive && 'text-destructive hover:bg-destructive/10 hover:text-destructive',
          )
        "
        :disabled="action.disabled"
        :title="hint(action)"
        :aria-label="action.label"
        :data-action="action.id"
        @click="emit('select', action.id)"
      >
        <component :is="action.icon" v-if="action.icon" />
        <span class="hidden @4xl:inline">{{ action.label }}</span>
      </Button>

      <DropdownMenuRoot v-if="more.length" :modal="false">
        <DropdownMenuTrigger as-child>
          <Button
            variant="outline"
            size="icon"
            :class="compact"
            :disabled="more.every((a) => a.disabled)"
            :title="t('explorer.toolbar.more')"
            :aria-label="t('explorer.toolbar.more')"
          >
            <Ellipsis />
            <span class="hidden @4xl:inline">{{ t('explorer.toolbar.more') }}</span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuPortal>
          <DropdownMenuContent align="start" :side-offset="4" :class="menu">
            <template v-for="(action, index) in more" :key="action.id">
              <DropdownMenuSeparator
                v-if="action.separated && index > 0"
                class="my-1 h-px bg-border"
              />
              <DropdownMenuItem
                :disabled="action.disabled"
                :class="
                  cn(
                    item,
                    action.destructive && 'text-destructive data-[highlighted]:bg-destructive/10',
                  )
                "
                @select="emit('select', action.id)"
              >
                <component :is="action.icon" v-if="action.icon" class="size-4" />
                <span class="flex-1">{{ action.label }}</span>
                <kbd v-if="action.shortcut" class="text-xs text-muted-foreground">{{
                  action.shortcut
                }}</kbd>
              </DropdownMenuItem>
            </template>
          </DropdownMenuContent>
        </DropdownMenuPortal>
      </DropdownMenuRoot>
    </div>

    <DropdownMenuRoot v-if="view === 'grid'" :modal="false">
      <DropdownMenuTrigger as-child>
        <Button
          variant="ghost"
          size="icon"
          :title="sortTitle"
          :aria-label="t('explorer.sort.label')"
        >
          <ArrowDownUp />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuPortal>
        <DropdownMenuContent align="end" :side-offset="4" :class="menu">
          <DropdownMenuItem
            v-for="key in SORT_KEYS"
            :key="key"
            :class="cn(item, sort.key === key && 'font-medium')"
            @select="emit('sort', key)"
          >
            <span class="flex-1">{{ t(`explorer.columns.${key}`) }}</span>
            <template v-if="sort.key === key">
              <ArrowDown v-if="sort.descending" class="size-3.5" />
              <ArrowUp v-else class="size-3.5" />
            </template>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>

    <div
      class="flex shrink-0 items-center gap-0.5 rounded-md border bg-background p-0.5"
      role="group"
      :aria-label="t('explorer.view.label')"
    >
      <button
        type="button"
        :class="toggle(view === 'list')"
        :aria-pressed="view === 'list'"
        :title="t('explorer.view.list')"
        :aria-label="t('explorer.view.list')"
        @click="emit('update:view', 'list')"
      >
        <List class="size-4" />
      </button>
      <button
        type="button"
        :class="toggle(view === 'grid')"
        :aria-pressed="view === 'grid'"
        :title="t('explorer.view.grid')"
        :aria-label="t('explorer.view.grid')"
        @click="emit('update:view', 'grid')"
      >
        <LayoutGrid class="size-4" />
      </button>
    </div>
  </div>
</template>
