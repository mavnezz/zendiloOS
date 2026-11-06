<script setup lang="ts">
import { Search } from 'lucide-vue-next'
import { PopoverContent, PopoverPortal, PopoverRoot, PopoverTrigger } from 'reka-ui'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { BrandMark } from '@/shared/ui/brand'
import { IconTile } from '@/shared/ui/icon-tile'

import type { AppDefinition } from './apps'

const props = defineProps<{ apps: AppDefinition[] }>()
const emit = defineEmits<{ open: [app: AppDefinition] }>()

const { t } = useI18n()
const open = ref(false)
const query = ref('')

const matches = computed(() => {
  const needle = query.value.trim().toLowerCase()
  return props.apps.filter((app) => t(`apps.${app.id}`).toLowerCase().includes(needle))
})

function launch(app: AppDefinition | undefined): void {
  if (!app) return
  emit('open', app)
  open.value = false
  query.value = ''
}
</script>

<!-- The start menu: every app this account may use, with a search. -->
<template>
  <PopoverRoot v-model:open="open">
    <PopoverTrigger
      class="flex h-8 items-center gap-2 rounded-md px-2 text-sm font-semibold hover:bg-foreground/[0.06] data-[state=open]:bg-foreground/[0.08]"
    >
      <BrandMark class="size-5" />
      <span class="hidden sm:inline">{{ t('desktop.apps') }}</span>
    </PopoverTrigger>
    <PopoverPortal>
      <PopoverContent
        side="top"
        align="start"
        :side-offset="8"
        class="z-[900] w-[22rem] rounded-xl border bg-card/95 p-3 shadow-2xl backdrop-blur-xl"
      >
        <label class="flex items-center gap-2 rounded-md border bg-background px-2">
          <Search class="size-4 text-muted-foreground" />
          <input
            v-model="query"
            class="h-8 flex-1 bg-transparent text-sm outline-none"
            :placeholder="t('desktop.searchApps')"
            :aria-label="t('desktop.searchApps')"
            autofocus
            @keydown.enter="launch(matches[0])"
          />
        </label>
        <div v-if="matches.length" class="mt-3 grid grid-cols-4 gap-1">
          <button
            v-for="app in matches"
            :key="app.id"
            type="button"
            class="flex flex-col items-center gap-1.5 rounded-lg p-2 text-xs hover:bg-accent"
            @click="launch(app)"
          >
            <IconTile :icon="app.icon" :tone="app.tone" size="lg" />
            {{ t(`apps.${app.id}`) }}
          </button>
        </div>
        <p v-else class="mt-3 text-center text-sm text-muted-foreground">
          {{ t('desktop.noApps') }}
        </p>
      </PopoverContent>
    </PopoverPortal>
  </PopoverRoot>
</template>
