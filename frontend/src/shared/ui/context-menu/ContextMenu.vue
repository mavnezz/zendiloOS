<script setup lang="ts">
import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuPortal,
  ContextMenuRoot,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from 'reka-ui'

import { cn } from '@/shared/lib/utils'

import type { MenuAction } from '.'

defineProps<{ actions: MenuAction[]; disabled?: boolean }>()
const emit = defineEmits<{ select: [id: string]; 'update:open': [open: boolean] }>()

const item =
  'flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-sm outline-none select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-[highlighted]:bg-accent'
</script>

<!--
  What can be done where the pointer is: a right click, or a long press on
  a touch screen, over the slot opens it.
-->
<template>
  <ContextMenuRoot :modal="false" @update:open="(open: boolean) => emit('update:open', open)">
    <ContextMenuTrigger as-child :disabled="disabled">
      <slot />
    </ContextMenuTrigger>
    <ContextMenuPortal>
      <ContextMenuContent
        class="z-[1100] min-w-52 rounded-xl border bg-popover p-1 text-popover-foreground shadow-lg"
        :collision-padding="8"
      >
        <template v-for="action in actions" :key="action.id">
          <ContextMenuSeparator v-if="action.separated" class="my-1 h-px bg-border" />
          <ContextMenuItem
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
          </ContextMenuItem>
        </template>
      </ContextMenuContent>
    </ContextMenuPortal>
  </ContextMenuRoot>
</template>
