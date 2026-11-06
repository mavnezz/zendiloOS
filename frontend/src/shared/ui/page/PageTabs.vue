<script setup lang="ts">
import { cn } from '@/shared/lib/utils'

defineProps<{ tabs: { id: string; label: string }[]; label: string }>()
const active = defineModel<string>({ required: true })
</script>

<!--
  Tabs within a page, under its header. The line under them is an inset
  shadow, not a border: the chosen tab's underline lies on it without
  reaching past the bottom, which would make the scrolling row 1px taller
  than itself and show a vertical scroll bar.
-->
<template>
  <div
    role="tablist"
    :aria-label="label"
    class="mb-6 flex gap-1 overflow-x-auto shadow-[inset_0_-1px_0_var(--color-border)]"
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
    </button>
  </div>
</template>
