<script setup lang="ts">
defineProps<{ title: string; description?: string }>()
</script>

<!-- A page's title, what it is about, and status or controls on the right. -->
<template>
  <header class="mb-5 flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
    <div class="grid gap-1">
      <h2 class="text-2xl font-semibold tracking-tight">{{ title }}</h2>
      <p v-if="description" class="text-sm text-muted-foreground">{{ description }}</p>
    </div>
    <div v-if="$slots.default" class="flex items-center gap-2">
      <slot />
    </div>
  </header>
</template>
