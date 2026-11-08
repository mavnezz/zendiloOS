<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

import type { NavGroup } from '.'

const props = defineProps<{
  /** Navigation; without it the page fills the window. */
  groups?: NavGroup[]
  /** No padding around the page: for tables and lists edge to edge. */
  flush?: boolean
  panelClass?: HTMLAttributes['class']
}>()
const section = defineModel<string>('section', { default: '' })
/**
 * An item was clicked, also the one shown as current: an app that shows a
 * page under an item (a plugin under Plugins) leads back by it.
 */
const emit = defineEmits<{ select: [id: string] }>()

function choose(id: string): void {
  section.value = id
  emit('select', id)
}

const items = computed(() => (props.groups ?? []).flatMap((group) => group.items))
</script>

<!--
  The inside of a window. Apps with pages get a navigation: a column on the
  left in wide windows, a scrollable row on top in narrow ones.
-->
<template>
  <div class="@container/app h-full">
    <div class="flex h-full flex-col @2xl/app:flex-row">
      <nav
        v-if="groups?.length"
        :aria-label="items.find((item) => item.id === section)?.label"
        class="flex shrink-0 gap-1 overflow-x-auto border-b bg-muted/40 p-2 @2xl/app:w-60 @2xl/app:flex-col @2xl/app:overflow-x-visible @2xl/app:overflow-y-auto @2xl/app:border-r @2xl/app:border-b-0"
      >
        <slot name="sidebar-top" />
        <div
          v-for="group in groups"
          :key="group.id"
          class="flex shrink-0 gap-1 @2xl/app:flex-col @2xl/app:not-first:mt-3"
        >
          <div
            v-if="group.label"
            class="hidden px-2 pb-1 text-xs font-medium text-muted-foreground @2xl/app:block"
          >
            {{ group.label }}
          </div>
          <button
            v-for="item in group.items"
            :key="item.id"
            type="button"
            :aria-current="section === item.id ? 'page' : undefined"
            :class="
              cn(
                'flex shrink-0 items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-sm whitespace-nowrap outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring @2xl/app:items-start @2xl/app:whitespace-normal',
                section === item.id && 'bg-primary/10 font-medium text-primary hover:bg-primary/15',
              )
            "
            @click="choose(item.id)"
          >
            <component :is="item.icon" v-if="item.icon" class="mt-0.5 size-4 shrink-0" />
            <span class="min-w-0 flex-1">{{ item.label }}</span>
            <span
              v-if="item.alert"
              class="mt-1.5 size-1.5 shrink-0 rounded-full bg-destructive"
              aria-hidden="true"
            />
            <span
              v-if="item.count !== undefined"
              class="mt-px rounded-full bg-foreground/[0.07] px-1.5 text-xs leading-4 text-muted-foreground tabular-nums"
            >
              {{ item.count }}
            </span>
          </button>
        </div>
        <div class="hidden @2xl/app:mt-auto @2xl/app:block">
          <slot name="sidebar-bottom" />
        </div>
      </nav>
      <div class="flex min-h-0 min-w-0 flex-1 flex-col">
        <slot name="tabs" />
        <div
          :class="
            cn(
              '@container min-h-0 flex-1 overflow-auto',
              // The padding as a variable: a page footer reaches over it.
              !flush &&
                'p-(--frame-padding) [--frame-padding:--spacing(4)] @3xl:[--frame-padding:--spacing(6)]',
              panelClass,
            )
          "
        >
          <slot />
        </div>
      </div>
    </div>
  </div>
</template>
