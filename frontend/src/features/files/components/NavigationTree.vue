<script setup lang="ts">
import { HardDrive, Star, Trash2 } from 'lucide-vue-next'
import { computed, inject } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Favourite, FileVolumeDto } from '@/shared/api/types'
import { cn } from '@/shared/lib/utils'

import { folderAt, HOME } from '../lib/location'
import { EXPLORER } from '../lib/tree'
import TreeBranch from './TreeBranch.vue'

/**
 * Where the explorer can go: the personal folder, the favourites, the
 * volumes with their folders, and the recycle bins. Removable drives get a
 * section of their own here once the NAS offers them.
 */
const props = defineProps<{
  home: boolean
  favourites: Favourite[]
  volumes: FileVolumeDto[]
}>()

const { t } = useI18n()
const explorer = inject(EXPLORER)!
const at = computed(() => explorer.current.value)

function favouriteName(favourite: Favourite): string {
  return favourite.path.split('/').pop() || favourite.share_name
}

const heading = 'px-2 pt-3 pb-1 text-xs font-medium text-muted-foreground'
const row = (active: boolean) =>
  cn(
    'flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring',
    active ? 'bg-primary/10 font-medium text-primary' : 'hover:bg-foreground/5',
  )
const favourites = computed(() => props.favourites)
</script>

<template>
  <nav class="grid content-start gap-0.5 p-2" :aria-label="t('explorer.navigation')">
    <ul v-if="home" role="tree" :aria-label="t('files.home')">
      <TreeBranch :share="HOME" path="" :name="t('files.home')" :depth="0" home />
    </ul>

    <template v-if="favourites.length">
      <p :class="heading">{{ t('files.favourites') }}</p>
      <button
        v-for="favourite in favourites"
        :key="`${favourite.share_id}:${favourite.path}`"
        type="button"
        :class="
          row(at.kind === 'folder' && at.share === favourite.share_id && at.path === favourite.path)
        "
        :title="`${favourite.share_name}/${favourite.path}`"
        @click="explorer.go(folderAt(favourite.share_id, favourite.path))"
      >
        <Star class="size-4 shrink-0 fill-current text-amber-500" />
        <span class="truncate">{{ favouriteName(favourite) }}</span>
      </button>
    </template>

    <template v-for="volume in volumes" :key="volume.volume">
      <button
        type="button"
        :class="cn(row(at.kind === 'volume' && at.volume === volume.volume), 'mt-3')"
        @click="explorer.go({ kind: 'volume', volume: volume.volume })"
      >
        <HardDrive class="size-4 shrink-0 text-muted-foreground" />
        <span class="truncate">{{
          t('explorer.volume', { n: volume.volume, pool: volume.pool })
        }}</span>
      </button>
      <ul role="tree" :aria-label="t('explorer.volume', { n: volume.volume, pool: volume.pool })">
        <TreeBranch
          v-for="folder in volume.folders"
          :key="folder.id"
          :share="folder.id"
          path=""
          :name="folder.name"
          :depth="1"
          :volume-folder="{ shared: folder.shared, access: folder.access ?? null }"
        />
      </ul>
    </template>

    <button
      type="button"
      :class="cn(row(at.kind === 'bin'), 'mt-3')"
      @click="explorer.go({ kind: 'bin' })"
    >
      <Trash2 class="size-4 shrink-0 text-muted-foreground" />
      <span class="truncate">{{ t('files.recycleBin') }}</span>
    </button>
  </nav>
</template>
