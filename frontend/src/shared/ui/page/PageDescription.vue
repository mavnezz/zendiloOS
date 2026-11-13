<script lang="ts">
import { ref } from 'vue'

const KEY = 'zendiloos.help'

function stored(): boolean {
  try {
    return localStorage.getItem(KEY) !== 'closed'
  } catch {
    return true // No storage: the help is simply there.
  }
}

/** One choice for every page: whoever folds the help away has read it. */
const open = ref(stored())
</script>

<script setup lang="ts">
import { ChevronDown, Info } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'

defineProps<{ title?: string }>()
const { t } = useI18n()

function toggle(): void {
  open.value = !open.value
  try {
    localStorage.setItem(KEY, open.value ? 'open' : 'closed')
  } catch {
    // Not kept: open again on the next visit.
  }
}
</script>

<!--
  What a page does, in plain words, above its controls: a NAS is run by
  people who are not administrators by trade, and a page of switches leaves
  them guessing. It folds away for those who know, on every page at once.
-->
<template>
  <section class="mb-6 max-w-3xl rounded-lg bg-muted/60 text-sm text-muted-foreground">
    <h3>
      <button
        type="button"
        class="flex w-full items-center gap-1.5 rounded-lg px-3 py-2 text-left font-medium text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
        :aria-expanded="open"
        @click="toggle"
      >
        <Info class="size-4 shrink-0 text-info" />
        <span class="flex-1">{{ title ?? t('common.description') }}</span>
        <ChevronDown :class="cn('size-4 text-muted-foreground transition', open && 'rotate-180')" />
      </button>
    </h3>
    <div v-show="open" class="space-y-2 px-3 pb-3">
      <slot />
    </div>
  </section>
</template>
