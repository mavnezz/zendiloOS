<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ class?: HTMLAttributes['class'] }>()
</script>

<template>
  <div data-slot="card-title" :class="cn('text-base leading-snug font-medium', props.class)">
    <slot />
  </div>
</template>
