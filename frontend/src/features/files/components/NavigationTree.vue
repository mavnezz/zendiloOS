<script setup lang="ts">
import { Star, Trash2 } from 'lucide-vue-next'
import { computed, inject, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Favourite, FileVolumeDto, RemovableDrive } from '@/shared/api/types'
import { formatBytes } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'

import { folderAt, HOME } from '../lib/location'
import { storeCollapsed, storedCollapsed } from '../lib/preferences'
import { EXPLORER } from '../lib/tree'
import PersonalFoldersBranch from './PersonalFoldersBranch.vue'
import SidebarSection from './SidebarSection.vue'
import TreeBranch from './TreeBranch.vue'

/**
 * Where the explorer can go, in sections that fold away: the person's own
 * files (the personal folder and the favourites), each volume with its
 * folders (and, for those who manage shares, the personal folders of
 * every account on it), the removable drives plugged in (for those who
 * manage storage), and the recycle bins.
 */
const props = defineProps<{
  home: boolean
  favourites: Favourite[]
  volumes: FileVolumeDto[]
  drives: RemovableDrive[]
}>()

const { t } = useI18n()
const explorer = inject(EXPLORER)!
const at = computed(() => explorer.current.value)

/** The sections folded away, remembered in this browser. */
const collapsed = ref(storedCollapsed())
function toggle(id: string): void {
  const next = new Set(collapsed.value)
  if (!next.delete(id)) next.add(id)
  collapsed.value = next
  storeCollapsed(next)
}
const open = (id: string) => !collapsed.value.has(id)

function favouriteName(favourite: Favourite): string {
  return favourite.path.split('/').pop() || favourite.share_name
}

/** A place in the tree; the one shown stands out. */
const row = (active: boolean) =>
  cn(
    'flex w-full items-center gap-2 rounded-md py-1 pr-2 text-left text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring',
    active
      ? 'bg-background font-medium text-primary shadow-xs dark:bg-foreground/10'
      : 'hover:bg-foreground/5',
  )
const favourites = computed(() => props.favourites)

/** A drive by its label, or its ID without one. */
function driveName(drive: RemovableDrive): string {
  return drive.label || drive.id
}

/** How full a mounted drive is, as a tooltip. */
function driveTitle(drive: RemovableDrive): string | undefined {
  if (drive.available_bytes === null || drive.available_bytes === undefined) return undefined
  return t('explorer.drive.free', {
    free: formatBytes(drive.available_bytes),
    size: formatBytes(drive.size_bytes),
  })
}
</script>

<template>
  <nav class="flex min-h-full flex-col gap-0.5 p-2" :aria-label="t('explorer.navigation')">
    <SidebarSection
      v-if="home || favourites.length"
      :title="t('explorer.sections.mine')"
      :open="open('mine')"
      @toggle="toggle('mine')"
    >
      <ul v-if="home" role="tree" :aria-label="t('files.home')">
        <TreeBranch :share="HOME" path="" :name="t('files.home')" :depth="1" home />
      </ul>
      <button
        v-for="favourite in favourites"
        :key="`${favourite.share_id}:${favourite.path}`"
        type="button"
        :class="
          cn(
            row(
              at.kind === 'folder' && at.share === favourite.share_id && at.path === favourite.path,
            ),
            'pl-10',
          )
        "
        :title="`${favourite.share_name}/${favourite.path}`"
        @click="explorer.go(folderAt(favourite.share_id, favourite.path))"
      >
        <Star class="size-4 shrink-0 fill-current text-amber-500" />
        <span class="truncate">{{ favouriteName(favourite) }}</span>
      </button>
    </SidebarSection>

    <SidebarSection
      v-for="volume in volumes"
      :key="volume.volume"
      :title="t('explorer.volume', { n: volume.volume, pool: volume.pool })"
      :open="open(`volume:${volume.volume}`)"
      navigable
      :active="at.kind === 'volume' && at.volume === volume.volume"
      @toggle="toggle(`volume:${volume.volume}`)"
      @navigate="explorer.go({ kind: 'volume', volume: volume.volume })"
    >
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
        <PersonalFoldersBranch
          v-if="volume.personal_folders.length"
          :volume="volume.volume"
          :folders="volume.personal_folders"
        />
      </ul>
    </SidebarSection>

    <SidebarSection
      v-if="drives.length"
      :title="t('explorer.drive.heading')"
      :open="open('drives')"
      @toggle="toggle('drives')"
    >
      <ul role="tree" :aria-label="t('explorer.drive.heading')">
        <TreeBranch
          v-for="drive in drives"
          :key="drive.id"
          :share="drive.id"
          path=""
          :name="driveName(drive)"
          :depth="1"
          :title="driveTitle(drive)"
          :drive="{ state: drive.state, problem: drive.problem ?? null }"
        />
      </ul>
    </SidebarSection>

    <div class="mt-auto pt-3">
      <button
        type="button"
        :class="cn(row(at.kind === 'bin'), 'pl-1.5')"
        @click="explorer.go({ kind: 'bin' })"
      >
        <Trash2 class="size-4 shrink-0 text-muted-foreground" />
        <span class="truncate">{{ t('files.recycleBin') }}</span>
      </button>
    </div>
  </nav>
</template>
