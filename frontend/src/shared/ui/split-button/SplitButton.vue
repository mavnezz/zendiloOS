<script setup lang="ts">
import { ChevronDown } from 'lucide-vue-next'
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
import type { Action } from '@/shared/ui/action-menu'
import { buttonVariants, type ButtonVariants } from '@/shared/ui/button'

withDefaults(
  defineProps<{
    label: string
    icon?: Component
    /** What else there is, behind the arrow. */
    actions: Action[]
    /** Names the arrow for screen readers. */
    menuLabel: string
    disabled?: boolean
    variant?: ButtonVariants['variant']
    size?: 'default' | 'sm'
  }>(),
  { icon: undefined, variant: 'outline', size: 'sm' },
)
const emit = defineEmits<{ click: []; select: [id: string] }>()

const item =
  'flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-sm outline-none select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-[highlighted]:bg-accent'
</script>

<!--
  A button with its usual action, and an arrow beside it for the others:
  "Shut down ▾" with forcing off and restarting in its menu.
-->
<template>
  <div class="inline-flex" role="group">
    <button
      type="button"
      :disabled="disabled"
      :class="cn(buttonVariants({ variant, size }), 'rounded-r-none')"
      @click="emit('click')"
    >
      <component :is="icon" v-if="icon" />
      {{ label }}
    </button>
    <DropdownMenuRoot>
      <DropdownMenuTrigger
        :aria-label="menuLabel"
        :class="
          cn(
            buttonVariants({ variant, size: size === 'sm' ? 'icon-sm' : 'icon' }),
            '-ml-px rounded-l-none',
          )
        "
      >
        <ChevronDown />
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
  </div>
</template>
