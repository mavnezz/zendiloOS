<script setup lang="ts">
import { ListFilter } from 'lucide-vue-next'
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Group, ShareRuleLevel } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Avatar } from '@/shared/ui/avatar'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { PageTabs } from '@/shared/ui/page'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useAccountActions, useAccounts, useAccountSettings, useShareRules } from '../api'
import { ruleOf } from '../lib/access'
import SharePermissions from './SharePermissions.vue'

/** Creates a group in three steps, or edits `group`. */
const props = defineProps<{ group?: Group; ownAccountId?: number }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useAccountActions()
const settings = useAccountSettings()
const notifications = useNotifications()
const { data: accounts } = useAccounts()
const { data: shares } = useShareRules(open)

const editing = computed(() => props.group !== undefined)
const STEPS = ['info', 'members', 'permissions'] as const
const step = ref(0)
const tab = ref<(typeof STEPS)[number]>('info')
const current = computed(() => (editing.value ? tab.value : STEPS[step.value]!))
const pending = ref(false)
const error = ref<string | null>(null)
const filter = ref('')

interface GroupForm {
  name: string
  description: string
  members: number[]
  rules: Record<number, ShareRuleLevel | null>
}

const form = reactive<GroupForm>({ name: '', description: '', members: [], rules: {} })

const ready = computed(() => Boolean(accounts.value && shares.value))
const filled = ref(false)
watch(
  [open, ready],
  ([isOpen, isReady]) => {
    if (!isOpen) filled.value = false
    else if (isReady && !filled.value) {
      Object.assign(form, {
        name: props.group?.name ?? '',
        description: props.group?.description ?? '',
        members: [...(props.group?.members ?? [])],
        rules: Object.fromEntries(
          (shares.value ?? []).map((share) => [
            share.id,
            ruleOf(share, 'group', props.group?.id ?? null),
          ]),
        ),
      })
      step.value = 0
      tab.value = 'info'
      error.value = null
      filter.value = ''
      filled.value = true
    }
  },
  { immediate: true },
)

const shown = computed(() => {
  const needle = filter.value.trim().toLowerCase()
  return (accounts.value ?? []).filter(
    (account) =>
      !needle ||
      account.name.includes(needle) ||
      (account.full_name ?? '').toLowerCase().includes(needle) ||
      (account.description ?? '').toLowerCase().includes(needle),
  )
})
const allShown = computed(
  () => shown.value.length > 0 && shown.value.every((account) => form.members.includes(account.id)),
)
function toggle(id: number, on: boolean): void {
  form.members = on
    ? [...new Set([...form.members, id])]
    : form.members.filter((member) => member !== id)
}
function toggleAll(on: boolean): void {
  for (const account of shown.value) toggle(account.id, on)
}

function next(): void {
  if (step.value === 0 && !form.name.trim()) {
    error.value = t('people.needName')
    return
  }
  error.value = null
  step.value += 1
}

async function finish(): Promise<void> {
  if (!form.name.trim()) {
    error.value = t('people.needName')
    return
  }
  pending.value = true
  error.value = null
  try {
    // Shares whose files follow changed members in the background.
    let following: string[] = []
    let group: Group
    if (props.group) {
      const updated = await actions.updateGroup(props.group.id, {
        description: form.description,
        members: form.members,
      })
      following = updated.pending_shares
      group = updated
    } else {
      group = await actions.createGroup({
        name: form.name.trim(),
        description: form.description || null,
        members: form.members,
      })
    }
    const rules = (shares.value ?? [])
      .filter(
        (share) =>
          (form.rules[share.id] ?? null) !== ruleOf(share, 'group', props.group?.id ?? null),
      )
      .map((share) => ({ share_id: share.id, rule: form.rules[share.id] ?? null }))
    try {
      await settings.setSubjectAccess('group', group.id, rules)
    } catch (failure) {
      open.value = false
      notifications.notify(
        'error',
        t('people.groupSavedPartly', { name: group.name }),
        problemMessage(failure),
      )
      return
    }
    notifications.notify(
      'success',
      props.group
        ? t('groups.saved', { name: group.name })
        : t('groups.created', { name: group.name }),
      following.length ? t('people.sharesFollow', { shares: following.join(', ') }) : undefined,
    )
    open.value = false
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <StepDialog
    v-model:open="open"
    :title="editing ? t('groups.editTitle', { name: group?.name }) : t('people.createGroup')"
    :close-label="t('desktop.close')"
    :steps="
      editing
        ? undefined
        : [t('people.groupInfo'), t('people.chooseMembers'), t('people.stepPermissions')]
    "
    :step="step"
  >
    <template v-if="editing" #tabs>
      <div class="px-5">
        <PageTabs
          v-model="tab"
          :label="t('groups.editTitle', { name: group?.name })"
          :tabs="[
            { id: 'info', label: t('people.groupInfo') },
            { id: 'members', label: t('groups.members') },
            { id: 'permissions', label: t('people.stepPermissions') },
          ]"
          class="mb-0"
        />
      </div>
    </template>

    <Alert v-if="error" variant="destructive" class="mb-3">{{ error }}</Alert>

    <form
      v-if="current === 'info'"
      class="grid gap-4"
      @submit.prevent="editing ? finish() : next()"
    >
      <div class="grid gap-2">
        <Label for="group-name"
          >{{ t('groups.name') }}<span class="text-destructive">*</span></Label
        >
        <Input
          id="group-name"
          v-model="form.name"
          :disabled="editing"
          autocomplete="off"
          autocapitalize="none"
          spellcheck="false"
          maxlength="32"
          :placeholder="t('people.namePlaceholder')"
        />
      </div>
      <div class="grid gap-2">
        <Label for="group-description">{{ t('groups.description') }}</Label>
        <Input
          id="group-description"
          v-model="form.description"
          autocomplete="off"
          maxlength="200"
        />
      </div>
      <button type="submit" class="hidden" />
    </form>

    <div v-else-if="current === 'members'" class="grid gap-3">
      <div class="relative max-w-xs">
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
      <Table>
        <TableHeader>
          <TableRow class="hover:bg-transparent">
            <TableHead class="w-10">
              <input
                type="checkbox"
                class="size-4 accent-primary"
                :checked="allShown"
                :aria-label="t('people.selectAll')"
                @change="toggleAll(($event.target as HTMLInputElement).checked)"
              />
            </TableHead>
            <TableHead>{{ t('users.name') }}</TableHead>
            <TableHead>{{ t('people.description') }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="account in shown" :key="account.id">
            <TableCell>
              <input
                type="checkbox"
                class="size-4 accent-primary"
                :checked="form.members.includes(account.id)"
                :aria-label="account.name"
                @change="toggle(account.id, ($event.target as HTMLInputElement).checked)"
              />
            </TableCell>
            <TableCell>
              <span class="flex items-center gap-2.5">
                <Avatar :name="account.name" size="sm" />
                <span class="font-medium">{{ account.name }}</span>
                <Badge v-if="account.id === ownAccountId">{{ t('people.me') }}</Badge>
              </span>
            </TableCell>
            <TableCell class="text-muted-foreground">{{ account.description ?? '–' }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <SharePermissions v-else v-model="form.rules" :shares="shares ?? []" />

    <template #footer>
      <Button variant="ghost" :disabled="pending" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button
        v-if="!editing && step > 0"
        variant="secondary"
        :disabled="pending"
        @click="step -= 1"
      >
        {{ t('people.back') }}
      </Button>
      <Button v-if="!editing && step < 2" :disabled="!filled" @click="next">{{
        t('people.next')
      }}</Button>
      <Button v-else :disabled="pending || !filled" @click="finish">
        {{ editing ? t('users.save') : t('people.finish') }}
      </Button>
    </template>
  </StepDialog>
</template>
