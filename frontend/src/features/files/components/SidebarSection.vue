<script setup lang="ts">
import { ChevronRight } from 'lucide-vue-next'
import { useId } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'

/**
 * A group of the places under a small heading that folds it away. With
 * `navigable`, the heading's name goes somewhere and only the chevron
 * folds; otherwise the whole heading does.
 */
defineProps<{
  title: string
  open: boolean
  navigable?: boolean
  /** Where the explorer is: the heading is marked. */
  active?: boolean
}>()
const emit = defineEmits<{ toggle: []; navigate: [] }>()

const { t } = useI18n()
const body = useId()

const focus = 'outline-none focus-visible:ring-2 focus-visible:ring-ring'
const label = 'min-w-0 truncate text-xs font-semibold tracking-wide'
</script>

<template>
  <section class="grid gap-0.5">
    <div class="flex items-center gap-0.5 pt-3 pb-0.5 text-muted-foreground">
      <template v-if="navigable">
        <button
          type="button"
          :class="
            cn('grid size-5 shrink-0 place-items-center rounded hover:bg-foreground/10', focus)
          "
          :aria-expanded="open"
          :aria-controls="body"
          :aria-label="
            open
              ? t('explorer.sections.fold', { name: title })
              : t('explorer.sections.unfold', { name: title })
          "
          @click="emit('toggle')"
        >
          <ChevronRight :class="cn('size-3.5 transition', open && 'rotate-90')" />
        </button>
        <button
          type="button"
          :class="
            cn(
              label,
              focus,
              'rounded px-1 py-0.5 text-left hover:text-foreground',
              active && 'text-primary',
            )
          "
          :aria-current="active ? 'location' : undefined"
          @click="emit('navigate')"
        >
          {{ title }}
        </button>
      </template>
      <button
        v-else
        type="button"
        :class="
          cn(
            'flex min-w-0 flex-1 items-center gap-0.5 rounded text-left hover:text-foreground',
            focus,
          )
        "
        :aria-expanded="open"
        :aria-controls="body"
        @click="emit('toggle')"
      >
        <span class="grid size-5 shrink-0 place-items-center">
          <ChevronRight :class="cn('size-3.5 transition', open && 'rotate-90')" />
        </span>
        <span :class="cn(label, 'px-1 py-0.5')">{{ title }}</span>
      </button>
    </div>
    <div v-show="open" :id="body" class="grid gap-0.5">
      <slot />
    </div>
  </section>
</template>
