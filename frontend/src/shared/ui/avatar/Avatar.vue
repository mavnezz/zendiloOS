<script setup lang="ts">
import { computed } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ name: string; size?: 'sm' | 'md' }>()

/** One of a few calm colours, always the same for a name. */
const TINTS = [
  'bg-sky-500/15 text-sky-700 dark:text-sky-300',
  'bg-violet-500/15 text-violet-700 dark:text-violet-300',
  'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300',
  'bg-amber-500/15 text-amber-700 dark:text-amber-300',
  'bg-rose-500/15 text-rose-700 dark:text-rose-300',
  'bg-teal-500/15 text-teal-700 dark:text-teal-300',
]
const tint = computed(() => {
  let hash = 0
  for (const char of props.name) hash = (hash * 31 + char.charCodeAt(0)) >>> 0
  return TINTS[hash % TINTS.length]
})
</script>

<!-- A round badge with the first letter of a name, for accounts and groups. -->
<template>
  <span
    aria-hidden="true"
    :class="
      cn(
        'grid shrink-0 place-items-center rounded-full font-semibold uppercase',
        size === 'sm' ? 'size-7 text-xs' : 'size-8 text-sm',
        tint,
      )
    "
  >
    {{ name.charAt(0) }}
  </span>
</template>
