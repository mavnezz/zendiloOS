<script setup lang="ts">
import type { HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

defineProps<{ title: string; description?: string; class?: HTMLAttributes['class'] }>()
</script>

<!-- A titled part of a panel, with room for actions on the right of the title. -->
<template>
  <section :class="cn('grid content-start gap-3', $props.class)">
    <header class="flex min-h-7 items-center gap-3">
      <div class="grid min-w-0 flex-1">
        <h3 class="text-sm font-semibold">{{ title }}</h3>
        <p v-if="description" class="text-xs text-muted-foreground">{{ description }}</p>
      </div>
      <slot name="actions" />
    </header>
    <slot />
  </section>
</template>
