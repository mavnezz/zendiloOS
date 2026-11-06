<script setup lang="ts">
import type { Component, HTMLAttributes } from 'vue'

import { cn } from '@/shared/lib/utils'

defineProps<{
  title?: string
  description?: string
  icon?: Component
  class?: HTMLAttributes['class']
}>()
</script>

<!-- A card on a page: a heading with its icon, then what belongs together. -->
<template>
  <section
    :class="
      cn(
        'flex flex-col gap-4 overflow-hidden rounded-xl bg-card py-4 text-sm text-card-foreground ring-1 ring-foreground/10',
        $props.class,
      )
    "
  >
    <header v-if="title || $slots.actions" class="flex items-start gap-3 px-4">
      <div class="grid min-w-0 flex-1 gap-1">
        <h3 v-if="title" class="flex items-center gap-2 text-base leading-snug font-medium">
          <component :is="icon" v-if="icon" class="size-4 text-muted-foreground" />
          {{ title }}
        </h3>
        <p v-if="description" class="text-sm text-muted-foreground">{{ description }}</p>
      </div>
      <slot name="actions" />
    </header>
    <div class="px-4">
      <slot />
    </div>
  </section>
</template>
