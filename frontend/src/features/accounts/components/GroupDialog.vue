<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Group } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useAccountActions, useAccounts } from '../api'

/** Creates a group, or edits `group` if given. */
const props = defineProps<{ group?: Group }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useAccountActions()
const notifications = useNotifications()
const { data: accounts } = useAccounts()

const name = ref('')
const description = ref('')
const members = ref<number[]>([])
const error = ref<string | null>(null)
const pending = ref(false)

const editing = computed(() => props.group !== undefined)

watch(open, (isOpen) => {
  if (!isOpen) return
  name.value = props.group?.name ?? ''
  description.value = props.group?.description ?? ''
  members.value = [...(props.group?.members ?? [])]
  error.value = null
})

async function submit(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    if (props.group) {
      await actions.updateGroup(props.group.id, {
        description: description.value,
        members: members.value,
      })
      notifications.notify('success', t('groups.saved', { name: props.group.name }))
    } else {
      const created = await actions.createGroup({
        name: name.value.trim(),
        description: description.value || null,
        members: members.value,
      })
      notifications.notify('success', t('groups.created', { name: created.name }))
    }
    open.value = false
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="editing ? t('groups.editTitle', { name: group?.name }) : t('groups.newTitle')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="submit">
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
      <div v-if="!editing" class="grid gap-2">
        <Label for="group-name">{{ t('groups.name') }}</Label>
        <Input
          id="group-name"
          v-model="name"
          autocomplete="off"
          autocapitalize="none"
          spellcheck="false"
          required
          maxlength="32"
        />
        <p class="text-xs text-muted-foreground">{{ t('setup.nameHint') }}</p>
      </div>
      <div class="grid gap-2">
        <Label for="group-description">{{ t('groups.description') }}</Label>
        <Input id="group-description" v-model="description" autocomplete="off" maxlength="200" />
      </div>
      <fieldset class="grid gap-2">
        <legend class="mb-2 text-sm font-medium">{{ t('groups.members') }}</legend>
        <p v-if="!accounts?.length" class="text-sm text-muted-foreground">{{ t('users.none') }}</p>
        <div class="grid max-h-56 gap-1 overflow-y-auto rounded-lg border p-2">
          <label
            v-for="account in accounts"
            :key="account.id"
            class="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent/60"
          >
            <input
              v-model="members"
              type="checkbox"
              :value="account.id"
              class="size-4 accent-primary"
            />
            <span class="flex-1 truncate">{{ account.full_name ?? account.name }}</span>
            <span class="text-xs text-muted-foreground">{{ account.name }}</span>
          </label>
        </div>
      </fieldset>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="pending">
          {{ editing ? t('users.save') : t('groups.create') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
