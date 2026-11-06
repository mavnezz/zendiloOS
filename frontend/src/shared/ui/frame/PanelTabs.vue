<script setup lang="ts">
import { cn } from '@/shared/lib/utils'

import type { TabItem } from '.'

defineProps<{ tabs: TabItem[] }>()
const active = defineModel<string>({ required: true })
</script>

<!--
  Tabs along the top of a page, the chosen one underlined; room on the
  right. The line under them is an inset shadow (see PageTabs).
-->
<template>
  <div
    class="flex shrink-0 items-end gap-1 overflow-x-auto px-4 pt-3 shadow-[inset_0_-1px_0_var(--color-border)] @3xl:px-6"
    role="tablist"
  >
    <button
      v-for="tab in tabs"
      :key="tab.id"
      type="button"
      role="tab"
      :aria-selected="active === tab.id"
      :class="
        cn(
          'shrink-0 border-b-2 px-3 pb-2.5 text-sm font-medium whitespace-nowrap outline-none transition hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring',
          active === tab.id
            ? 'border-primary text-primary'
            : 'border-transparent text-muted-foreground',
        )
      "
      @click="active = tab.id"
    >
      {{ tab.label }}
      <span
        v-if="tab.count !== undefined"
        class="ml-1 text-xs text-muted-foreground tabular-nums"
        >{{ tab.count }}</span
      >
    </button>
    <div class="ml-auto flex items-center gap-2 self-center pb-2">
      <slot />
    </div>
  </div>
</template>
