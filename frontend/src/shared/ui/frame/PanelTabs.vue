<script setup lang="ts">
import { cn } from '@/shared/lib/utils'

import type { TabItem } from '.'

defineProps<{ tabs: TabItem[] }>()
const active = defineModel<string>({ required: true })
</script>

<!-- Tabs along the top of a panel, the chosen one underlined. -->
<template>
  <div class="flex shrink-0 items-end gap-5 overflow-x-auto border-b px-5" role="tablist">
    <button
      v-for="tab in tabs"
      :key="tab.id"
      type="button"
      role="tab"
      :aria-selected="active === tab.id"
      :class="
        cn(
          'relative flex h-10 shrink-0 items-center gap-1.5 text-sm transition-colors',
          active === tab.id
            ? 'font-medium text-primary after:absolute after:inset-x-0 after:bottom-0 after:h-0.5 after:rounded-full after:bg-primary'
            : 'text-muted-foreground hover:text-foreground',
        )
      "
      @click="active = tab.id"
    >
      {{ tab.label }}
      <span v-if="tab.count !== undefined" class="text-xs text-muted-foreground tabular-nums">{{
        tab.count
      }}</span>
    </button>
    <div class="ml-auto flex items-center gap-2 self-center">
      <slot />
    </div>
  </div>
</template>
