<script setup lang="ts">
import { Pencil, Trash2, UsersRound } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Group } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

import { useAccountActions, useAccounts, useGroups } from '../api'
import GroupDialog from './GroupDialog.vue'

const { t } = useI18n()
const actions = useAccountActions()
const notifications = useNotifications()
const { data: groups, error, isPending } = useGroups()
const { data: accounts } = useAccounts()

const creating = ref(false)
const editing = ref<Group | null>(null)
const deleting = ref<Group | null>(null)

const names = computed(() => new Map(accounts.value?.map((a) => [a.id, a.name]) ?? []))

async function remove(): Promise<void> {
  const group = deleting.value
  if (!group) return
  try {
    await actions.deleteGroup(group.id)
    notifications.notify('success', t('groups.deleted', { name: group.name }))
  } catch (failure) {
    notifications.notify('error', t('groups.deleteFailed'), problemMessage(failure))
  }
}
</script>

<template>
  <div class="grid gap-3">
    <div class="flex items-center justify-between gap-2">
      <p class="text-sm text-muted-foreground">{{ t('groups.intro') }}</p>
      <Button size="sm" @click="creating = true">
        <UsersRound />
        {{ t('groups.new') }}
      </Button>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !groups?.length" class="text-sm text-muted-foreground">
      {{ t('groups.none') }}
    </p>
    <ul class="grid gap-2 @3xl:grid-cols-2">
      <li v-for="group in groups" :key="group.id" class="grid gap-2 rounded-xl border bg-card p-3">
        <div class="flex items-start gap-2">
          <div class="min-w-0 flex-1">
            <p class="truncate font-medium">{{ group.name }}</p>
            <p v-if="group.description" class="truncate text-xs text-muted-foreground">
              {{ group.description }}
            </p>
          </div>
          <Button
            variant="ghost"
            size="icon"
            :title="t('users.edit')"
            :aria-label="t('users.edit')"
            @click="editing = group"
          >
            <Pencil />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            :title="t('users.delete')"
            :aria-label="t('users.delete')"
            @click="deleting = group"
          >
            <Trash2 />
          </Button>
        </div>
        <ul v-if="group.members.length" class="flex flex-wrap gap-1.5">
          <li
            v-for="member in group.members"
            :key="member"
            class="rounded-md bg-secondary px-2 py-0.5 text-xs"
          >
            {{ names.get(member) ?? `#${member}` }}
          </li>
        </ul>
        <p v-else class="text-xs text-muted-foreground">{{ t('groups.noMembers') }}</p>
      </li>
    </ul>

    <GroupDialog v-model:open="creating" />
    <GroupDialog
      :open="editing !== null"
      :group="editing ?? undefined"
      @update:open="(open) => !open && (editing = null)"
    />
    <ConfirmDialog
      :open="deleting !== null"
      :title="t('groups.deleteTitle', { name: deleting?.name })"
      :description="t('groups.deleteDescription')"
      :confirm-label="t('users.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open) => !open && (deleting = null)"
      @confirm="remove"
    />
  </div>
</template>
