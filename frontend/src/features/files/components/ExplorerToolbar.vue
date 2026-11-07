<script setup lang="ts">
import type { MenuAction } from '@/shared/ui/context-menu'
import { cn } from '@/shared/lib/utils'

/** The same actions the context menu offers, as buttons. */
defineProps<{ actions: MenuAction[]; label: string }>()
const emit = defineEmits<{ select: [id: string] }>()
</script>

<template>
  <div
    role="toolbar"
    :aria-label="label"
    class="flex flex-wrap items-center gap-0.5 border-b px-2 py-1"
  >
    <template v-for="action in actions" :key="action.id">
      <span v-if="action.separated" class="mx-1 h-5 w-px bg-border" aria-hidden="true" />
      <button
        type="button"
        :class="
          cn(
            'inline-flex h-8 items-center gap-1.5 rounded-md px-2 text-sm outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-4',
            action.destructive && 'text-destructive hover:bg-destructive/10',
          )
        "
        :disabled="action.disabled"
        :title="action.shortcut ? `${action.label} (${action.shortcut})` : action.label"
        :aria-label="action.label"
        @click="emit('select', action.id)"
      >
        <component :is="action.icon" v-if="action.icon" />
        <span class="hidden @4xl:inline">{{ action.label }}</span>
      </button>
    </template>
  </div>
</template>
