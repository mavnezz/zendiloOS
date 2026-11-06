<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ class?: HTMLAttributes['class'] }>()
</script>

<template>
  <thead
    :class="
      cn(
        '[&_th]:bg-muted/60 [&_th:first-child]:rounded-l-md [&_th:last-child]:rounded-r-md',
        props.class,
      )
    "
  >
    <slot />
  </thead>
</template>
