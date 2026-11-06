<script setup lang="ts">
defineProps<{ label: string; for?: string }>()
</script>

<!-- One setting: its name on the left, the control and what it does on the right. -->
<template>
  <div class="grid gap-x-6 gap-y-1.5 sm:grid-cols-[11rem_1fr] sm:items-start">
    <label :for="$props.for" class="pt-1.5 text-sm text-muted-foreground">{{ label }}</label>
    <div class="grid min-w-0 gap-1.5">
      <slot />
    </div>
  </div>
</template>
