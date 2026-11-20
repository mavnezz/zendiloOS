<script setup lang="ts">
import { DatabaseBackup, Import } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { FoundPool, NamedPool, OtherPool } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

import { claimPool, useAbsentPools, useRefreshStorage } from '../api'
import ImportPoolDialog from './ImportPoolDialog.vue'

/**
 * The pools the configuration names that are not imported, as after a
 * restore on a machine installed afresh, and the other pools on the disks:
 * imported only when asked. A pool imported under a name the configuration
 * names, but another pool than that one, is taken as the configuration's
 * once confirmed. For administrators (`storage.manage`) only.
 */
const { t } = useI18n()
const { data: absent } = useAbsentPools(true)
const refresh = useRefreshStorage()
const importing = ref<{ found: FoundPool; known: NamedPool | null } | null>(null)
const claiming = ref<OtherPool | null>(null)
const claimError = ref<string | null>(null)

const others = computed(() => absent.value?.not_the_configurations ?? [])
const shown = computed(
  () =>
    !!absent.value &&
    (absent.value.known.length > 0 ||
      absent.value.missing.length > 0 ||
      absent.value.foreign.length > 0 ||
      others.value.length > 0),
)

async function claim(other: OtherPool): Promise<void> {
  claimError.value = null
  try {
    await claimPool(other.pool.name)
    await refresh()
  } catch (failure) {
    claimError.value = problemMessage(failure)
  }
}
</script>

<template>
  <section
    v-if="shown && absent"
    class="grid gap-3 rounded-lg border border-dashed p-4"
    aria-labelledby="absent-pools-title"
  >
    <header class="flex items-center gap-2">
      <DatabaseBackup class="size-4 shrink-0 text-muted-foreground" />
      <h3 id="absent-pools-title" class="text-sm font-semibold">
        {{ t('storage.imports.title') }}
      </h3>
    </header>
    <template v-if="absent.known.length">
      <p class="text-xs text-muted-foreground">{{ t('storage.imports.intro') }}</p>
      <p class="text-xs text-muted-foreground">{{ t('storage.imports.hostId') }}</p>
      <ul class="grid gap-2">
        <li
          v-for="entry in absent.known"
          :key="entry.found.id"
          class="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md bg-muted/50 px-3 py-2"
        >
          <span class="font-mono text-sm font-semibold">{{ entry.pool.name }}</span>
          <span class="text-xs text-muted-foreground">
            {{
              t('storage.imports.volume', {
                volume: entry.pool.volume,
                path: entry.pool.mount_point,
              })
            }}
            ·
            {{
              entry.pool.shares.length
                ? t('storage.imports.shares', { names: entry.pool.shares.join(', ') })
                : t('storage.imports.noShares')
            }}
          </span>
          <span class="flex-1" />
          <Badge v-if="entry.found.other_host" variant="warning">
            {{ t('storage.imports.otherHost') }}
          </Badge>
          <Badge
            v-if="!entry.found.can_import"
            variant="destructive"
            :title="entry.found.action ?? ''"
          >
            {{ t('storage.imports.cannotImport') }}
          </Badge>
          <Button v-else size="sm" @click="importing = { found: entry.found, known: entry.pool }">
            <Import />
            {{ t('storage.imports.import') }}
          </Button>
        </li>
      </ul>
    </template>
    <template v-if="others.length">
      <h4 class="text-xs font-semibold">{{ t('storage.imports.othersTitle') }}</h4>
      <p class="text-xs text-muted-foreground">{{ t('storage.imports.othersIntro') }}</p>
      <Alert v-if="claimError" variant="destructive">{{ claimError }}</Alert>
      <ul class="grid gap-2">
        <li
          v-for="other in others"
          :key="other.pool.name"
          class="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md bg-muted/50 px-3 py-2"
        >
          <span class="font-mono text-sm font-semibold">{{ other.pool.name }}</span>
          <span class="text-xs text-muted-foreground">
            {{
              t('storage.imports.volume', {
                volume: other.pool.volume,
                path: other.pool.mount_point,
              })
            }}
          </span>
          <span class="flex-1" />
          <Button size="sm" variant="outline" @click="claiming = other">
            {{ t('storage.imports.claim') }}
          </Button>
        </li>
      </ul>
    </template>
    <template v-if="absent.missing.length">
      <h4 class="text-xs font-semibold">{{ t('storage.imports.missingTitle') }}</h4>
      <ul class="grid gap-1 text-xs text-muted-foreground">
        <li v-for="pool in absent.missing" :key="pool.name">
          {{ t('storage.imports.missing', { name: pool.name, volume: pool.volume }) }}
        </li>
      </ul>
    </template>
    <template v-if="absent.foreign.length">
      <h4 class="text-xs font-semibold">{{ t('storage.imports.foreignTitle') }}</h4>
      <p class="text-xs text-muted-foreground">{{ t('storage.imports.foreignIntro') }}</p>
      <ul class="grid gap-2">
        <li
          v-for="found in absent.foreign"
          :key="found.id"
          class="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md bg-muted/50 px-3 py-2"
        >
          <span class="font-mono text-sm font-semibold">{{ found.name }}</span>
          <span class="text-xs text-muted-foreground">{{ found.state }}</span>
          <span class="flex-1" />
          <Badge v-if="found.other_host" variant="warning">
            {{ t('storage.imports.otherHost') }}
          </Badge>
          <Badge v-if="!found.can_import" variant="destructive" :title="found.action ?? ''">
            {{ t('storage.imports.cannotImport') }}
          </Badge>
          <Button v-else size="sm" variant="outline" @click="importing = { found, known: null }">
            <Import />
            {{ t('storage.imports.adopt') }}
          </Button>
        </li>
      </ul>
    </template>
    <ConfirmDialog
      v-if="claiming"
      :open="true"
      :title="t('storage.imports.claimTitle', { name: claiming.pool.name })"
      :description="t('storage.imports.claimDescription', { name: claiming.pool.name })"
      :confirm-label="t('storage.imports.claim')"
      :cancel-label="t('desktop.cancel')"
      @confirm="claiming && claim(claiming)"
      @update:open="(open) => !open && (claiming = null)"
    />
    <ImportPoolDialog
      v-if="importing"
      :open="true"
      :found="importing.found"
      :known="importing.known"
      :free-volume="absent.free_volume ?? null"
      @update:open="(open) => !open && (importing = null)"
    />
  </section>
</template>
