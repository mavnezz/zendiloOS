<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

import { type StatTone, statTones } from '.'

withDefaults(
  defineProps<{
    label: string
    value: string
    /** A line under the value, e.g. "of 31 GiB". */
    hint?: string
    tone?: StatTone
    class?: HTMLAttributes['class']
  }>(),
  { tone: 'muted', hint: undefined, class: undefined },
)
</script>

<!-- One figure with its name, marked by a coloured bar on the left. -->
<template>
  <div :class="cn('grid content-start gap-0.5 border-l-2 pl-3', statTones[tone], $props.class)">
    <span class="text-xs text-muted-foreground">{{ label }}</span>
    <span class="text-xl leading-7 font-semibold tracking-tight tabular-nums">{{ value }}</span>
    <span v-if="hint" class="text-xs text-muted-foreground tabular-nums">{{ hint }}</span>
    <slot />
  </div>
</template>
