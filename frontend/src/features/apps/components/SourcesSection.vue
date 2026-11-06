<script setup lang="ts">
import { Plus, RefreshCw, ShieldAlert, Trash2 } from 'lucide-vue-next'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useSourceActions, useSources } from '../api'

const { t } = useI18n()
const { data: sources, error } = useSources()
const actions = useSourceActions()

const address = ref('')
const understood = ref(false)
const failure = ref<string | null>(null)
const busy = ref(false)

async function run(action: () => Promise<unknown>): Promise<void> {
  failure.value = null
  busy.value = true
  try {
    await action()
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}

function add(): Promise<void> {
  return run(async () => {
    await actions.add(address.value.trim())
    address.value = ''
    understood.value = false
  })
}
</script>

<!-- The catalogs apps come from: the community catalog, and others like HACS. -->
<template>
  <div class="grid gap-5">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>

    <div class="flex items-center gap-2">
      <p class="flex-1 text-sm text-muted-foreground">{{ t('appStore.sources.hint') }}</p>
      <Button size="sm" variant="outline" :disabled="busy" @click="run(actions.refreshAll)">
        <RefreshCw />
        {{ t('appStore.sources.refresh') }}
      </Button>
    </div>

    <ul class="grid gap-2">
      <li
        v-for="source in sources"
        :key="source.id"
        class="grid gap-1 rounded-xl border bg-card p-3 text-sm"
      >
        <div class="flex items-center gap-2">
          <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ source.address }}</span>
          <Badge v-if="source.official">{{ t('appStore.sources.official') }}</Badge>
          <Badge v-else variant="outline" class="border-amber-500/60">{{
            t('appStore.unofficial')
          }}</Badge>
          <Button
            v-if="!source.official"
            variant="ghost"
            size="icon"
            :title="t('appStore.sources.remove')"
            :aria-label="t('appStore.sources.remove')"
            :disabled="busy"
            @click="run(() => actions.remove(source.id))"
          >
            <Trash2 />
          </Button>
        </div>
        <p class="text-xs text-muted-foreground">
          <template v-if="source.refreshed_at">
            {{ t('appStore.sources.read', { time: formatDateTime(source.refreshed_at * 1000) }) }}
          </template>
          <template v-else>{{ t('appStore.sources.notRead') }}</template>
          <template v-if="source.skipped">
            · {{ t('appStore.sources.skipped', { count: source.skipped }, source.skipped) }}
          </template>
        </p>
        <p v-if="source.refresh_error" class="text-xs text-destructive">
          {{ source.refresh_error }}
        </p>
      </li>
    </ul>

    <form class="grid gap-2 rounded-xl border p-3" @submit.prevent="add">
      <Label for="source-address">{{ t('appStore.sources.add') }}</Label>
      <Input
        id="source-address"
        v-model="address"
        placeholder="github.com/owner/repository"
        autocomplete="off"
        required
      />
      <p class="text-xs text-muted-foreground">{{ t('appStore.sources.addressHint') }}</p>
      <label class="flex items-start gap-2 text-sm">
        <input v-model="understood" type="checkbox" class="mt-0.5 size-4 accent-primary" />
        <span>
          <ShieldAlert class="mr-1 inline size-4 text-amber-600" />
          {{ t('appStore.sources.warning') }}
        </span>
      </label>
      <div class="flex justify-end">
        <Button type="submit" size="sm" :disabled="busy || !understood || !address.trim()">
          <Plus />
          {{ t('appStore.sources.addButton') }}
        </Button>
      </div>
    </form>
  </div>
</template>
