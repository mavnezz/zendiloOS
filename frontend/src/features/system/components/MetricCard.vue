<script setup lang="ts">
import type { Component } from 'vue'

defineProps<{ title: string; icon: Component }>()
</script>

<template>
  <section class="flex flex-col gap-3 rounded-xl border bg-card p-4">
    <h3 class="flex items-center gap-2 text-sm font-medium text-muted-foreground">
      <component :is="icon" class="size-4 text-primary" />
      {{ title }}
    </h3>
    <slot />
  </section>
</template>
