<script setup lang="ts">
import { ChevronRight, FolderLock, Users } from 'lucide-vue-next'
import { computed, inject, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PersonalFolderDto } from '@/shared/api/types'
import { cn } from '@/shared/lib/utils'

import { HOME } from '../lib/location'
import { EXPLORER } from '../lib/tree'
import TreeBranch from './TreeBranch.vue'

/**
 * The personal folders of every account on a volume, for those who manage
 * shares: their own opens as the personal folder does, the others' are
 * private to their accounts and only listed.
 */
const props = defineProps<{ volume: number; folders: PersonalFolderDto[] }>()

const { t } = useI18n()
const explorer = inject(EXPLORER)!
const expanded = ref(false)
const title = computed(() => t('explorer.homes.title'))

const here = computed(() => {
  const at = explorer.current.value
  return at.kind === 'homes' && at.volume === props.volume
})
function shown(name: string): boolean {
  const at = explorer.current.value
  return at.kind === 'private' && at.volume === props.volume && at.name === name
}
// Showing one of them from elsewhere (the list on the right) opens the branch.
watch(
  () => explorer.current.value,
  (at) => {
    if (at.kind === 'private' && at.volume === props.volume) expanded.value = true
  },
)

const row = (active: boolean) =>
  cn(
    'flex items-center gap-1 rounded-md py-1 pr-2 text-sm',
    active ? 'bg-primary/10 font-medium text-primary' : 'hover:bg-foreground/5',
  )
</script>

<template>
  <li role="treeitem" :aria-expanded="expanded" :aria-selected="here">
    <!-- No menu here: nothing to do with the folders of others but look. -->
    <div :class="row(here)" style="padding-left: 16px" @contextmenu.stop.prevent>
      <button
        type="button"
        class="grid size-5 shrink-0 place-items-center rounded text-muted-foreground hover:bg-foreground/10"
        :aria-label="
          expanded ? t('explorer.collapse', { name: title }) : t('explorer.expand', { name: title })
        "
        @click="expanded = !expanded"
      >
        <ChevronRight :class="cn('size-3.5 transition', expanded && 'rotate-90')" />
      </button>
      <button
        type="button"
        class="flex min-w-0 flex-1 items-center gap-1.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
        @click="explorer.go({ kind: 'homes', volume })"
      >
        <Users class="size-4 shrink-0 text-sky-500" />
        <span class="truncate">{{ title }}</span>
      </button>
    </div>
    <ul v-if="expanded" role="group">
      <template v-for="folder in folders" :key="folder.account_id">
        <TreeBranch v-if="folder.own" :share="HOME" path="" :name="folder.name" :depth="2" home />
        <li v-else role="treeitem" :aria-selected="shown(folder.name)">
          <div
            :class="cn(row(shown(folder.name)), !shown(folder.name) && 'text-muted-foreground')"
            style="padding-left: 28px"
            @contextmenu.stop.prevent
          >
            <span class="size-5 shrink-0" aria-hidden="true" />
            <button
              type="button"
              class="flex min-w-0 flex-1 items-center gap-1.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
              :title="t('explorer.homes.private', { name: folder.name })"
              @click="explorer.go({ kind: 'private', volume, name: folder.name })"
            >
              <FolderLock class="size-4 shrink-0 fill-amber-400/30 text-amber-500" />
              <span class="truncate">{{ folder.name }}</span>
              <span class="sr-only">{{ t('explorer.homes.private', { name: folder.name }) }}</span>
            </button>
          </div>
        </li>
      </template>
    </ul>
  </li>
</template>
