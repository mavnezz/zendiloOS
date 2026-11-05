<script setup lang="ts">
import { FolderOpen, Pencil, Plus, Trash2 } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Share } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useShareActions, useShares, useSubjects } from '../api'
import ShareDialog from './ShareDialog.vue'

const { t } = useI18n()
const { data: shares, error, isPending, refetch } = useShares()
const { accounts, groups } = useSubjects()
const actions = useShareActions()
const notifications = useNotifications()
const jobs = useJobsStore()

const creating = ref(false)
const editing = ref<Share | null>(null)
const deleting = ref<Share | null>(null)
const confirm = ref('')
const deleteError = ref<string | null>(null)

const names = computed(() => ({
  account: new Map(accounts.data.value?.map((a) => [a.id, a.name]) ?? []),
  group: new Map(groups.data.value?.map((g) => [g.id, g.name]) ?? []),
}))

watch(
  () =>
    jobs.list.filter((job) => job.kind.startsWith('sharing.') && job.state !== 'running').length,
  () => void refetch(),
)

async function remove(): Promise<void> {
  if (!deleting.value) return
  deleteError.value = null
  try {
    await actions.remove(deleting.value.id, confirm.value)
    notifications.notify('info', t('shares.deleting', { name: deleting.value.name }))
    deleting.value = null
  } catch (failure) {
    deleteError.value = problemMessage(failure)
  }
}
</script>

<!-- Shares on the volumes: who may use them over SMB, NFS and in Files. -->
<template>
  <div class="grid content-start gap-3 p-4 @xl:p-6">
    <div class="flex items-center justify-between gap-2">
      <p class="text-sm text-muted-foreground">{{ t('shares.intro') }}</p>
      <Button size="sm" @click="creating = true">
        <Plus />
        {{ t('shares.new') }}
      </Button>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !shares?.length" class="text-sm text-muted-foreground">
      {{ t('shares.none') }}
    </p>
    <section
      v-for="share in shares"
      :key="share.id"
      class="grid gap-2 rounded-xl border bg-card p-4"
    >
      <header class="flex flex-wrap items-center gap-3">
        <FolderOpen class="size-5 shrink-0 text-primary" />
        <div class="min-w-0 flex-1">
          <h3 class="font-medium">{{ share.name }}</h3>
          <p class="truncate text-xs text-muted-foreground">
            <span class="font-mono">{{ share.path }}</span>
            <template v-if="share.description"> · {{ share.description }}</template>
          </p>
        </div>
        <Badge v-if="share.nfs.length" variant="outline">NFS</Badge>
        <Badge v-if="share.recycle_bin" variant="secondary">{{ t('shares.recycleBin') }}</Badge>
        <Button
          variant="ghost"
          size="icon"
          :title="t('users.edit')"
          :aria-label="t('users.edit')"
          @click="editing = share"
        >
          <Pencil />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          :title="t('users.delete')"
          :aria-label="t('users.delete')"
          @click="((deleting = share), (confirm = ''))"
        >
          <Trash2 />
        </Button>
      </header>
      <ul v-if="share.access.length" class="flex flex-wrap gap-1.5">
        <li
          v-for="rule in share.access"
          :key="`${rule.kind}:${rule.id}`"
          class="rounded-md bg-secondary px-2 py-0.5 text-xs"
        >
          {{ names[rule.kind].get(rule.id) ?? `#${rule.id}` }}
          <template v-if="rule.kind === 'group'"> ({{ t('shares.group') }})</template>
          · {{ t(`shares.levels.${rule.access}`) }}
        </li>
      </ul>
      <p v-else class="text-xs text-destructive">{{ t('shares.noAccess') }}</p>
    </section>

    <ShareDialog v-model:open="creating" />
    <ShareDialog
      :open="editing !== null"
      :share="editing ?? undefined"
      @update:open="(open) => !open && (editing = null)"
    />
    <ModalDialog
      :open="deleting !== null"
      :title="t('shares.deleteTitle', { name: deleting?.name })"
      :description="t('shares.deleteDescription', { path: deleting?.path })"
      :close-label="t('desktop.close')"
      @update:open="(open) => !open && (deleting = null)"
    >
      <form class="grid gap-4" @submit.prevent="remove">
        <Alert v-if="deleteError" variant="destructive">{{ deleteError }}</Alert>
        <div class="grid gap-2">
          <Label for="share-confirm">{{
            t('storage.create.confirm', { name: deleting?.name })
          }}</Label>
          <Input id="share-confirm" v-model="confirm" autocomplete="off" spellcheck="false" />
        </div>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" @click="deleting = null">
            {{ t('desktop.cancel') }}
          </Button>
          <Button type="submit" variant="destructive" :disabled="confirm.trim() !== deleting?.name">
            {{ t('shares.deleteSubmit') }}
          </Button>
        </div>
      </form>
    </ModalDialog>
  </div>
</template>
