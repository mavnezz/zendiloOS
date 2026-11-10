<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

defineProps<{
  /** Name and value; `mono` for technical values. */
  items: { label: string; value: string; mono?: boolean }[]
  class?: HTMLAttributes['class']
}>()
</script>

<!-- Name and value rows, the value on the right: system details. -->
<template>
  <dl :class="cn('grid gap-2.5 text-sm', $props.class)">
    <div v-for="item in items" :key="item.label" class="flex justify-between gap-4">
      <dt class="shrink-0 text-muted-foreground">{{ item.label }}</dt>
      <dd :class="cn('text-right font-medium wrap-anywhere', item.mono && 'font-mono text-xs')">
        {{ item.value }}
      </dd>
    </div>
  </dl>
</template>
