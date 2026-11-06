<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

import type { InfoItem } from '.'

defineProps<{ items: InfoItem[]; class?: HTMLAttributes['class'] }>()
</script>

<!-- Names and values in two columns; technical values in a fixed-width font. -->
<template>
  <dl
    :class="
      cn('grid grid-cols-[minmax(7rem,max-content)_1fr] gap-x-6 gap-y-2 text-sm', $props.class)
    "
  >
    <template v-for="item in items" :key="item.label">
      <dt class="text-muted-foreground">{{ item.label }}</dt>
      <dd :class="cn('min-w-0 truncate', item.mono && 'font-mono text-[12px]')" :title="item.value">
        {{ item.value }}
      </dd>
    </template>
  </dl>
</template>
