<script setup lang="ts">
import { Ellipsis } from 'lucide-vue-next'
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from 'reka-ui'
import type { Component } from 'vue'

import { cn } from '@/shared/lib/utils'

export interface Action {
  id: string
  label: string
  icon?: Component
  /** Shown in red, after a line. */
  destructive?: boolean
  disabled?: boolean
}

defineProps<{ actions: Action[]; label: string }>()
const emit = defineEmits<{ select: [id: string] }>()

const item =
  'flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-sm outline-none select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-[highlighted]:bg-accent'
</script>

<!-- What can be done with one row of a table, behind "…". -->
<template>
  <DropdownMenuRoot>
    <DropdownMenuTrigger
      :aria-label="label"
      class="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50"
    >
      <Ellipsis class="size-4" />
    </DropdownMenuTrigger>
    <DropdownMenuPortal>
      <DropdownMenuContent
        align="end"
        :side-offset="4"
        class="z-[1100] min-w-44 rounded-xl border bg-popover p-1 text-popover-foreground shadow-lg"
      >
        <template v-for="action in actions" :key="action.id">
          <DropdownMenuSeparator v-if="action.destructive" class="my-1 h-px bg-border" />
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
            {{ action.label }}
          </DropdownMenuItem>
        </template>
      </DropdownMenuContent>
    </DropdownMenuPortal>
  </DropdownMenuRoot>
</template>
