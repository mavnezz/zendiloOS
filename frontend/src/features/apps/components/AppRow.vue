<script setup lang="ts">
import { Blocks } from 'lucide-vue-next'
import { ref } from 'vue'

/** `icon`: where the app's icon is; a block when it cannot be loaded. `plain`: no line under it. */
defineProps<{ name: string; subtitle?: string; icon?: string; plain?: boolean }>()
const emit = defineEmits<{ open: [] }>()
const broken = ref(false)
</script>

<!-- One app in a list: its icon, name and kind, and what can be done with it. -->
<template>
  <div class="flex min-w-0 items-center gap-3 py-3.5" :class="plain ? '' : 'border-b'">
    <button
      type="button"
      class="flex min-w-0 flex-1 items-center gap-3 rounded-lg text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
      @click="emit('open')"
    >
      <span
        class="grid size-12 shrink-0 place-items-center overflow-hidden rounded-xl border bg-card"
      >
        <Blocks v-if="!icon || broken" class="size-6 text-muted-foreground" />
        <img
          v-else
          :src="icon"
          alt=""
          class="size-full object-cover"
          loading="lazy"
          @error="broken = true"
        />
      </span>
      <span class="grid min-w-0 flex-1 gap-0.5">
        <span class="truncate font-semibold">{{ name }}</span>
        <span v-if="subtitle" class="truncate text-xs text-muted-foreground">{{ subtitle }}</span>
      </span>
    </button>
    <div class="flex shrink-0 items-center gap-1">
      <slot />
    </div>
  </div>
</template>
