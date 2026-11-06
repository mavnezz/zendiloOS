<script setup lang="ts">
import { ListFilter, Pencil, Trash2, UsersRound } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Group } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { ActionMenu } from '@/shared/ui/action-menu'
import { Alert } from '@/shared/ui/alert'
import { Avatar } from '@/shared/ui/avatar'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { Input } from '@/shared/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useAccountActions, useAccounts, useGroups } from '../api'
import GroupWizard from './GroupWizard.vue'

defineProps<{ ownAccountId: number | undefined }>()

const { t } = useI18n()
const actions = useAccountActions()
const notifications = useNotifications()
const { data: groups, error, isPending } = useGroups()
const { data: accounts } = useAccounts()

const filter = ref('')
const creating = ref(false)
const editing = ref<Group | null>(null)
const deleting = ref<Group | null>(null)

const shown = computed(() => {
  const needle = filter.value.trim().toLowerCase()
  return (groups.value ?? []).filter(
    (group) =>
      !needle ||
      group.name.includes(needle) ||
      (group.description ?? '').toLowerCase().includes(needle),
  )
})
const names = (group: Group) =>
  group.members.map((id) => accounts.value?.find((account) => account.id === id)?.name ?? `#${id}`)

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

<!-- Every group with its members: who gets the same access at once. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <Button @click="creating = true">
        <UsersRound />
        {{ t('people.createGroup') }}
      </Button>
      <div class="relative ml-auto w-full max-w-60">
        <ListFilter
          class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="filter"
          class="pl-8"
          :placeholder="t('people.filter')"
          :aria-label="t('people.filter')"
        />
      </div>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <div class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('groups.name') }}</TableHead>
            <TableHead class="hidden @2xl:table-cell">{{ t('groups.description') }}</TableHead>
            <TableHead>{{ t('groups.members') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('people.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="group in shown" :key="group.id">
            <TableCell>
              <span class="flex items-center gap-2.5">
                <span
                  class="grid size-8 shrink-0 place-items-center rounded-lg bg-primary/10 text-primary"
                >
                  <UsersRound class="size-4" />
                </span>
                <span class="font-medium">{{ group.name }}</span>
              </span>
            </TableCell>
            <TableCell class="hidden max-w-64 truncate text-muted-foreground @2xl:table-cell">
              {{ group.description ?? '–' }}
            </TableCell>
            <TableCell>
              <span v-if="!group.members.length" class="text-muted-foreground">{{
                t('groups.noMembers')
              }}</span>
              <span v-else class="flex items-center gap-2" :title="names(group).join(', ')">
                <span class="flex -space-x-1.5">
                  <Avatar
                    v-for="name in names(group).slice(0, 4)"
                    :key="name"
                    :name="name"
                    size="sm"
                    class="ring-2 ring-card"
                  />
                </span>
                <span class="text-xs text-muted-foreground tabular-nums">{{
                  group.members.length
                }}</span>
              </span>
            </TableCell>
            <TableCell class="text-right">
              <ActionMenu
                :actions="[
                  { id: 'edit', label: t('users.edit'), icon: Pencil },
                  { id: 'delete', label: t('users.delete'), icon: Trash2, destructive: true },
                ]"
                :label="t('people.actionsFor', { name: group.name })"
                @select="(action) => (action === 'edit' ? (editing = group) : (deleting = group))"
              />
            </TableCell>
          </TableRow>
          <TableRow v-if="!isPending && !shown.length" class="hover:bg-transparent">
            <TableCell colspan="4" class="py-8 text-center text-muted-foreground">
              {{ filter ? t('people.nothingFound') : t('groups.none') }}
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <GroupWizard v-model:open="creating" :own-account-id="ownAccountId" />
    <GroupWizard
      :open="editing !== null"
      :group="editing ?? undefined"
      :own-account-id="ownAccountId"
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
