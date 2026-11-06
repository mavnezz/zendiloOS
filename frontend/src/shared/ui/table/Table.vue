<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

const props = defineProps<{ class?: HTMLAttributes['class'] }>()
</script>

<template>
  <div class="relative w-full overflow-x-auto">
    <table
      :class="cn('w-full caption-bottom border-separate border-spacing-0 text-sm', props.class)"
    >
      <slot />
    </table>
  </div>
</template>
