<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

import { IconTile } from '../icon-tile'
import type { NavGroup } from '.'

const props = defineProps<{
  /** Side navigation; without it the panel fills the window. */
  groups?: NavGroup[]
  /** No padding inside the panel: for tables and lists edge to edge. */
  flush?: boolean
  panelClass?: HTMLAttributes['class']
}>()
const section = defineModel<string>('section', { default: '' })

const items = computed(() => (props.groups ?? []).flatMap((group) => group.items))
</script>

<!--
  The inside of a window: side navigation on the window's frame and a white
  panel for the content, with optional tabs above it. Under 640 px of width
  the navigation becomes a row above the panel.
-->
<template>
  <div class="flex h-full flex-col @2xl:flex-row">
    <template v-if="groups?.length">
      <nav
        class="hidden w-52 shrink-0 flex-col gap-3 overflow-y-auto px-2 pt-0.5 pb-2 @2xl:flex"
        :aria-label="items.find((item) => item.id === section)?.label"
      >
        <slot name="sidebar-top" />
        <div v-for="group in groups" :key="group.id" class="grid gap-px">
          <p
            v-if="group.label"
            class="px-2.5 pb-1 text-[11px] font-medium tracking-wide text-muted-foreground uppercase"
          >
            {{ group.label }}
          </p>
          <button
            v-for="item in group.items"
            :key="item.id"
            type="button"
            :class="
              cn(
                'group flex h-8 items-center gap-2.5 rounded-md px-2.5 text-left text-sm transition-colors',
                section === item.id
                  ? 'bg-card font-medium text-primary shadow-xs'
                  : 'text-foreground/85 hover:bg-chrome-hover',
              )
            "
            :aria-current="section === item.id ? 'page' : undefined"
            @click="section = item.id"
          >
            <IconTile v-if="item.icon && item.tone" :icon="item.icon" :tone="item.tone" size="sm" />
            <component
              :is="item.icon"
              v-else-if="item.icon"
              :class="
                cn(
                  'size-4 shrink-0',
                  section === item.id ? 'text-primary' : 'text-muted-foreground',
                )
              "
            />
            <span class="min-w-0 flex-1 truncate">{{ item.label }}</span>
            <span v-if="item.alert" class="size-1.5 rounded-full bg-destructive" />
            <span
              v-if="item.count !== undefined"
              class="min-w-5 rounded-full bg-foreground/[0.06] px-1.5 text-center text-[11px] leading-5 text-muted-foreground tabular-nums"
            >
              {{ item.count }}
            </span>
          </button>
        </div>
        <div class="mt-auto">
          <slot name="sidebar-bottom" />
        </div>
      </nav>
      <nav class="flex shrink-0 gap-1 overflow-x-auto px-1.5 pb-1.5 @2xl:hidden">
        <button
          v-for="item in items"
          :key="item.id"
          type="button"
          :class="
            cn(
              'flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2.5 text-xs',
              section === item.id
                ? 'bg-card font-medium text-primary shadow-xs'
                : 'hover:bg-chrome-hover',
            )
          "
          :aria-current="section === item.id ? 'page' : undefined"
          @click="section = item.id"
        >
          <component :is="item.icon" v-if="item.icon" class="size-3.5" />
          {{ item.label }}
        </button>
      </nav>
    </template>
    <div
      :class="
        cn(
          'mx-1.5 mb-1.5 flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-black/[0.06] bg-card shadow-xs dark:border-white/[0.06]',
          groups?.length && '@2xl:ml-0',
        )
      "
    >
      <slot name="tabs" />
      <div
        :class="cn('@container/panel min-h-0 flex-1 overflow-auto', !flush && 'p-5', panelClass)"
      >
        <slot />
      </div>
    </div>
  </div>
</template>
