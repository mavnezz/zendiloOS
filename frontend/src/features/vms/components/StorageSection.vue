<script setup lang="ts">
import { Database, LibraryBig, Plus, Server, Trash2 } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VmLocation, VmPlugin } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatPercent } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { Meter } from '@/shared/ui/meter'
import { NativeSelect } from '@/shared/ui/native-select'

import { useLocationActions, useVmLocations } from '../api'
import { freePools, poolFill, removalBlocked, vmShare } from '../lib/storage'

/** Where the VMs' disks lie: a location per pool, the first with the image library. */
const props = defineProps<{ plugin: VmPlugin }>()

const { t } = useI18n()
const { data: locations, error, isPending } = useVmLocations()
const actions = useLocationActions()

const failure = ref<string | null>(null)
const adding = ref(false)
const pool = ref('')
const removing = ref<VmLocation | null>(null)
const confirm = ref('')
const pending = ref(false)

const free = computed(() => freePools(props.plugin))
watch(adding, (open) => {
  if (open) pool.value = free.value[0]?.pool ?? ''
})
watch(removing, () => (confirm.value = ''))

const HEALTH: Record<string, 'success' | 'warning' | 'destructive' | 'secondary'> = {
  online: 'success',
  degraded: 'warning',
  faulted: 'destructive',
  unavailable: 'destructive',
  suspended: 'destructive',
}

function health(location: VmLocation): string {
  return t(`vms.storage.health.${location.health ?? 'unknown'}`)
}

function blocked(location: VmLocation): string | null {
  const why = removalBlocked(location, locations.value ?? [])
  return why && t(`vms.storage.blocked.${why}`)
}

async function add(): Promise<void> {
  if (!pool.value) return
  pending.value = true
  failure.value = null
  try {
    await actions.add(pool.value)
    adding.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
    adding.value = false
  } finally {
    pending.value = false
  }
}

async function remove(): Promise<void> {
  const location = removing.value
  if (!location) return
  pending.value = true
  failure.value = null
  try {
    await actions.remove(location.pool, confirm.value.trim())
    removing.value = null
  } catch (problem) {
    failure.value = problemMessage(problem)
    removing.value = null
  } finally {
    pending.value = false
  }
}
</script>

<!-- The VMs' storage: each location with its pool's space, the VMs on it and their share. -->
<template>
  <div class="grid content-start gap-6">
    <header class="flex flex-wrap items-center gap-3">
      <div class="grid min-w-0 flex-1 gap-0.5">
        <h2 class="display text-xl">{{ t('vms.sections.storage') }}</h2>
        <p class="text-sm text-muted-foreground">{{ t('vms.storage.description') }}</p>
      </div>
      <Button :disabled="!free.length" data-testid="vm-location-add" @click="adding = true">
        <Plus />
        {{ t('vms.storage.add') }}
      </Button>
    </header>

    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive" data-testid="vm-storage-failure">{{
      failure
    }}</Alert>

    <div v-if="isPending" class="h-32 animate-pulse rounded-xl bg-muted" />
    <ul v-else class="grid gap-3">
      <li
        v-for="location in locations"
        :key="location.pool"
        :data-location="location.pool"
        class="grid gap-3 rounded-xl border bg-card p-4"
      >
        <div class="flex flex-wrap items-center gap-3">
          <span
            class="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground"
            aria-hidden="true"
          >
            <Database class="size-5" />
          </span>
          <div class="grid min-w-0 flex-1 gap-0.5">
            <span class="font-medium">{{ t('vms.storage.pool', { pool: location.pool }) }}</span>
            <span class="text-xs text-muted-foreground">
              {{ t('vms.storage.volume', { n: location.volume }) }}
            </span>
          </div>
          <Badge v-if="location.library" variant="info">
            <LibraryBig />
            {{ t('vms.storage.library') }}
          </Badge>
          <Badge :variant="HEALTH[location.health ?? ''] ?? 'secondary'">{{
            health(location)
          }}</Badge>
          <Button
            variant="ghost"
            size="sm"
            :disabled="Boolean(blocked(location))"
            :title="blocked(location) ?? undefined"
            :aria-label="t('vms.storage.removeLabel', { pool: location.pool })"
            @click="removing = location"
          >
            <Trash2 />
            {{ t('vms.storage.remove') }}
          </Button>
        </div>

        <div class="grid gap-1.5">
          <Meter
            v-if="poolFill(location) !== null"
            :value="poolFill(location)!"
            kind="usage"
            :label="t('vms.storage.ofPool')"
          />
          <p class="flex flex-wrap justify-between gap-x-4 text-xs text-muted-foreground">
            <span v-if="location.pool_used_bytes != null && location.available_bytes != null">{{
              t('vms.storage.used', {
                used: formatBytes(location.pool_used_bytes),
                free: formatBytes(location.available_bytes),
              })
            }}</span>
            <span v-if="location.used_bytes != null">{{
              t('vms.storage.vmsTake', { size: formatBytes(location.used_bytes) })
            }}</span>
            <span>{{
              location.images
                ? t(
                    'vms.storage.images',
                    { n: location.images, size: formatBytes(location.images_bytes ?? 0) },
                    location.images,
                  )
                : t('vms.storage.noImages')
            }}</span>
          </p>
        </div>

        <div class="grid gap-2">
          <h3 class="text-sm font-medium">{{ t('vms.storage.vms') }}</h3>
          <p v-if="!location.vms.length" class="text-sm text-muted-foreground">
            {{ t('vms.storage.noVms') }}
          </p>
          <ul v-else class="grid gap-1.5">
            <li
              v-for="vm in location.vms"
              :key="vm.id"
              :data-vm="vm.id"
              class="grid gap-1 rounded-lg border px-3 py-2 sm:grid-cols-[1fr_auto] sm:items-center"
            >
              <span class="flex min-w-0 items-center gap-2 text-sm">
                <Server class="size-4 shrink-0 text-muted-foreground" />
                <span class="truncate font-medium">{{ vm.name }}</span>
                <span class="text-xs text-muted-foreground">{{
                  t(
                    'vms.storage.disks',
                    { disks: vm.disks.map((slot) => slot + 1).join(', ') },
                    vm.disks.length,
                  )
                }}</span>
              </span>
              <span class="text-xs text-muted-foreground tabular-nums">
                {{
                  t('vms.storage.takes', {
                    used:
                      vm.used_bytes != null ? formatBytes(vm.used_bytes) : t('vms.storage.notRead'),
                    size: formatBytes(vm.size_bytes),
                  })
                }}
                <template v-if="vmShare(vm, location) !== null">
                  ·
                  {{
                    t('vms.storage.share', { percent: formatPercent(vmShare(vm, location)! * 100) })
                  }}
                </template>
              </span>
              <Meter
                v-if="vmShare(vm, location) !== null"
                class="sm:col-span-2"
                :value="vmShare(vm, location)!"
                kind="progress"
                :label="
                  t('vms.storage.share', { percent: formatPercent(vmShare(vm, location)! * 100) })
                "
              />
            </li>
          </ul>
        </div>
      </li>
    </ul>

    <ModalDialog
      v-model:open="adding"
      :title="t('vms.storage.addDialog.title')"
      :description="t('vms.storage.addDialog.description')"
      :close-label="t('desktop.close')"
    >
      <form class="grid gap-4" @submit.prevent="add">
        <p v-if="!free.length" class="text-sm text-muted-foreground">
          {{ t('vms.storage.addDialog.none') }}
        </p>
        <div v-else class="grid gap-2">
          <Label for="vm-location-pool">{{ t('vms.storage.addDialog.pool') }}</Label>
          <NativeSelect id="vm-location-pool" v-model="pool">
            <option v-for="choice in free" :key="choice.pool" :value="choice.pool">
              {{ t('appStore.placeVolume', { number: choice.volume, pool: choice.pool }) }} ·
              {{ formatBytes(choice.available_bytes) }}
            </option>
          </NativeSelect>
        </div>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" @click="adding = false">
            {{ t('desktop.cancel') }}
          </Button>
          <Button type="submit" :disabled="pending || !pool">
            {{ t('vms.storage.addDialog.submit') }}
          </Button>
        </div>
      </form>
    </ModalDialog>

    <ModalDialog
      :open="Boolean(removing)"
      :title="t('vms.storage.removeDialog.title', { pool: removing?.pool ?? '' })"
      :description="t('vms.storage.removeDialog.description')"
      :close-label="t('desktop.close')"
      @update:open="(open: boolean) => !open && (removing = null)"
    >
      <form class="grid gap-4" @submit.prevent="remove">
        <div class="grid gap-2">
          <Label for="vm-location-confirm">{{
            t('vms.storage.removeDialog.confirm', { pool: removing?.pool ?? '' })
          }}</Label>
          <Input
            id="vm-location-confirm"
            v-model="confirm"
            autocomplete="off"
            spellcheck="false"
            required
          />
        </div>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" @click="removing = null">
            {{ t('desktop.cancel') }}
          </Button>
          <Button
            type="submit"
            variant="destructive"
            :disabled="pending || confirm.trim() !== removing?.pool"
          >
            {{ t('vms.storage.removeDialog.submit') }}
          </Button>
        </div>
      </form>
    </ModalDialog>
  </div>
</template>
