<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'

import { saveInventory, useInventory, useRefreshStorage } from '../api'

/** `canManage`: may note what it is (`storage.manage`). */
const props = defineProps<{ serial: string | null | undefined; canManage: boolean }>()

const { t } = useI18n()
const { data: inventory } = useInventory()
const refresh = useRefreshStorage()

const entry = computed(() => inventory.value?.find((disk) => disk.serial === props.serial))
const form = ref({ purchased: '', warranty_until: '', vendor: '', note: '' })
const failure = ref<string | null>(null)
const saved = ref(false)
const saving = ref(false)

watch(
  entry,
  (current) => {
    form.value = {
      purchased: current?.purchased ?? '',
      warranty_until: current?.warranty_until ?? '',
      vendor: current?.vendor ?? '',
      note: current?.note ?? '',
    }
  },
  { immediate: true },
)

async function save(): Promise<void> {
  if (!props.serial) return
  failure.value = null
  saved.value = false
  saving.value = true
  try {
    await saveInventory(props.serial, { ...form.value })
    await refresh()
    saved.value = true
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}
</script>

<!-- When and where a disk was bought and until when its warranty holds, kept by its serial number. -->
<template>
  <section class="grid gap-2">
    <h4 class="font-medium">{{ t('storage.inventory.title') }}</h4>
    <p v-if="!serial" class="text-sm text-muted-foreground">
      {{ t('storage.inventory.noSerial') }}
    </p>
    <form v-else class="grid gap-3" @submit.prevent="save">
      <fieldset :disabled="!canManage" class="grid grid-cols-1 gap-3 @xl:grid-cols-3">
        <label class="grid gap-1 text-xs">
          {{ t('storage.inventory.purchased') }}
          <Input id="inventory-purchased" v-model="form.purchased" type="date" />
        </label>
        <label class="grid gap-1 text-xs">
          {{ t('storage.inventory.warrantyUntil') }}
          <Input id="inventory-warranty" v-model="form.warranty_until" type="date" />
        </label>
        <label class="grid gap-1 text-xs">
          {{ t('storage.inventory.vendor') }}
          <Input id="inventory-vendor" v-model="form.vendor" :maxlength="64" />
        </label>
        <label class="grid gap-1 text-xs @xl:col-span-3">
          {{ t('storage.inventory.note') }}
          <textarea
            id="inventory-note"
            v-model="form.note"
            rows="2"
            maxlength="500"
            class="rounded-md border bg-background px-2.5 py-1.5 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50 dark:bg-input/30"
          />
        </label>
      </fieldset>
      <div class="flex flex-wrap items-center gap-2">
        <p v-if="entry" class="min-w-0 flex-1 text-xs text-muted-foreground">
          {{
            t('storage.inventory.seen', {
              first: formatDateTime(entry.first_seen * 1000),
              last: formatDateTime(entry.last_seen * 1000),
            })
          }}
        </p>
        <span v-else class="flex-1" />
        <p v-if="saved" class="text-xs text-success" role="status">
          {{ t('storage.inventory.saved') }}
        </p>
        <p v-if="failure" class="text-xs text-destructive" role="alert">{{ failure }}</p>
        <Button v-if="canManage" type="submit" size="sm" :disabled="saving || !entry">
          {{ t('storage.inventory.save') }}
        </Button>
      </div>
    </form>
  </section>
</template>
