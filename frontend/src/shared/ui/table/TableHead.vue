<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ class?: HTMLAttributes['class'] }>()
</script>

<template>
  <th
    :class="
      cn(
        'h-8 px-3 text-left align-middle text-xs font-medium whitespace-nowrap text-muted-foreground',
        props.class,
      )
    "
  >
    <slot />
  </th>
</template>
