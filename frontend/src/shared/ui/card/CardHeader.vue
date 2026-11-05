<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ class?: HTMLAttributes['class'] }>()
</script>

<template>
  <div
    data-slot="card-header"
    :class="cn('grid auto-rows-min items-start gap-1.5 px-6', props.class)"
  >
    <slot />
  </div>
</template>
