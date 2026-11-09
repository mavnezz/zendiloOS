<script setup lang="ts">
import { Dices, Eye, EyeOff } from 'lucide-vue-next'
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Account, AccountRole, Share, ShareRuleLevel } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { PageTabs } from '@/shared/ui/page'

import {
  useAccountActions,
  useAccountSettings,
  useGroups,
  useHomes,
  usePasswordPolicy,
  useShareRules,
} from '../api'
import { effectiveAccess, ruleOf } from '../lib/access'
import { generatePassword } from '../lib/password'
import SharePermissions from './SharePermissions.vue'

const ROLES: AccountRole[] = ['user', 'readonly', 'admin']
const GIB = 1024 ** 3

/** Creates an account in two steps, or edits `account`. */
const props = defineProps<{ account?: Account; own?: boolean }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useAccountActions()
const settings = useAccountSettings()
const notifications = useNotifications()
const { data: groups } = useGroups()
const { data: shares } = useShareRules(open)
const { data: homes } = useHomes(open)
const { data: policy } = usePasswordPolicy()

const editing = computed(() => props.account !== undefined)
const step = ref(0)
const tab = ref('info')
const pending = ref(false)
const error = ref<string | null>(null)
const showPassword = ref(false)

interface AccountForm {
  name: string
  fullName: string
  password: string
  confirm: string
  email: string
  description: string
  role: AccountRole
  enabled: boolean
  passwordLocked: boolean
  groups: number[]
  home: boolean
  /** In the unit below; empty: no limit. */
  quota: string | number
  unit: 'GB' | 'TB'
  rules: Record<number, ShareRuleLevel | null>
}

const form = reactive<AccountForm>({
  name: '',
  fullName: '',
  password: '',
  confirm: '',
  email: '',
  description: '',
  role: 'user',
  enabled: true,
  passwordLocked: false,
  groups: [],
  home: false,
  quota: '',
  unit: 'GB',
  rules: {},
})

const homesOn = computed(() => Boolean(homes.value?.location))
const memberOf = (id: number | undefined) =>
  (groups.value ?? [])
    .filter((group) => id !== undefined && group.members.includes(id))
    .map((g) => g.id)
const homeOf = (id: number | undefined) => homes.value?.homes.find((home) => home.account_id === id)

function reset(): void {
  const account = props.account
  const home = homeOf(account?.id)
  const quota = home?.quota_bytes ?? null
  Object.assign(form, {
    name: account?.name ?? '',
    fullName: account?.full_name ?? '',
    password: '',
    confirm: '',
    email: account?.email ?? '',
    description: account?.description ?? '',
    role: account?.role ?? 'user',
    enabled: !(account?.disabled ?? false),
    passwordLocked: account?.password_locked ?? false,
    groups: memberOf(account?.id),
    home: account ? Boolean(home) : homesOn.value,
    quota: quota ? (quota % 1024 ** 4 === 0 ? quota / 1024 ** 4 : quota / GIB) : '',
    unit: quota && quota % 1024 ** 4 === 0 ? 'TB' : 'GB',
    rules: Object.fromEntries(
      (shares.value ?? []).map((share) => [
        share.id,
        ruleOf(share, 'account', account?.id ?? null),
      ]),
    ),
  })
  step.value = 0
  tab.value = 'info'
  error.value = null
  showPassword.value = false
}
/** The form is filled once, when the lists it starts from are there. */
const ready = computed(() => Boolean(groups.value && shares.value && homes.value))
const filled = ref(false)
watch(
  [open, ready],
  ([isOpen, isReady]) => {
    if (!isOpen) filled.value = false
    else if (isReady && !filled.value) {
      reset()
      filled.value = true
    }
  },
  { immediate: true },
)

const minLength = computed(() => policy.value?.min_length ?? 10)
const quotaBytes = computed(() => {
  const amount = Number(form.quota)
  if (form.quota === '' || !Number.isFinite(amount) || amount <= 0) return null
  return Math.round(amount * (form.unit === 'TB' ? 1024 ** 4 : GIB))
})
const problems = computed(() => {
  const found: string[] = []
  if (!editing.value) {
    if (!form.name.trim()) found.push(t('people.needName'))
    if (form.password.length < minLength.value)
      found.push(t('people.tooShort', { min: minLength.value }))
    else if (form.password !== form.confirm) found.push(t('people.mismatch'))
  }
  if (form.home && quotaBytes.value !== null && quotaBytes.value < GIB)
    found.push(t('people.quotaTooSmall'))
  return found
})

function generate(): void {
  const password = generatePassword(Math.max(16, minLength.value))
  form.password = password
  form.confirm = password
  showPassword.value = true
}

const effective = (share: Share, rule: ShareRuleLevel | null) =>
  effectiveAccess(share, props.account?.id ?? -1, form.groups, rule)

function toggleGroup(id: number, on: boolean): void {
  form.groups = on ? [...form.groups, id] : form.groups.filter((group) => group !== id)
}

/** Group memberships, home and share rules, after the account itself. */
async function follow(id: number): Promise<void> {
  const before = memberOf(props.account?.id)
  for (const group of groups.value ?? []) {
    const wanted = form.groups.includes(group.id)
    if (wanted === before.includes(group.id)) continue
    const members = wanted
      ? [...group.members, id]
      : group.members.filter((member) => member !== id)
    await actions.updateGroup(group.id, { members })
  }
  const home = homeOf(props.account?.id)
  const homeChanged =
    form.home !== Boolean(home) || (form.home && (home?.quota_bytes ?? null) !== quotaBytes.value)
  if (homesOn.value && homeChanged) {
    await settings.setAccountHome(id, form.home, form.home ? quotaBytes.value : null)
  }
  const rules = (shares.value ?? [])
    .filter(
      (share) =>
        (form.rules[share.id] ?? null) !== ruleOf(share, 'account', props.account?.id ?? null),
    )
    .map((share) => ({ share_id: share.id, rule: form.rules[share.id] ?? null }))
  await settings.setSubjectAccess('account', id, rules)
}

async function finish(): Promise<void> {
  if (problems.value.length) {
    error.value = problems.value[0]!
    return
  }
  pending.value = true
  error.value = null
  try {
    if (props.account) {
      await actions.updateAccount(props.account.id, {
        full_name: form.fullName,
        description: form.description,
        email: form.email,
        ...(props.own
          ? {}
          : { role: form.role, disabled: !form.enabled, password_locked: form.passwordLocked }),
      })
      await follow(props.account.id)
      notifications.notify('success', t('users.saved', { name: props.account.name }))
    } else {
      const created = await actions.createAccount({
        name: form.name.trim(),
        full_name: form.fullName || null,
        description: form.description || null,
        email: form.email || null,
        role: form.role,
        password: form.password,
        password_locked: form.passwordLocked,
      })
      try {
        await follow(created.id)
      } catch (failure) {
        // The account exists; what failed can be fixed by editing it.
        open.value = false
        notifications.notify(
          'error',
          t('people.createdPartly', { name: created.name }),
          problemMessage(failure),
        )
        return
      }
      notifications.notify(
        'success',
        t('users.created', { name: created.name }),
        form.passwordLocked ? undefined : t('users.createdHint'),
      )
    }
    open.value = false
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}

function next(): void {
  if (problems.value.length) {
    error.value = problems.value[0]!
    return
  }
  error.value = null
  step.value = 1
}
</script>

<template>
  <StepDialog
    v-model:open="open"
    :title="editing ? t('users.editTitle', { name: account?.name }) : t('people.createUser')"
    :close-label="t('desktop.close')"
    :steps="editing ? undefined : [t('people.stepInfo'), t('people.stepPermissions')]"
    :step="step"
  >
    <template v-if="editing" #tabs>
      <div class="px-5">
        <PageTabs
          v-model="tab"
          :label="t('users.editTitle', { name: account?.name })"
          :tabs="[
            { id: 'info', label: t('people.stepInfo') },
            { id: 'permissions', label: t('people.stepPermissions') },
          ]"
          class="mb-0"
        />
      </div>
    </template>

    <Alert v-if="error" variant="destructive" class="mb-3">{{ error }}</Alert>

    <form
      v-if="editing ? tab === 'info' : step === 0"
      class="grid gap-x-4 gap-y-3.5 sm:grid-cols-[10rem_1fr] sm:items-center"
      @submit.prevent="editing ? finish() : next()"
    >
      <Label for="account-name" class="sm:justify-end"
        >{{ t('users.name') }}<span class="text-destructive">*</span></Label
      >
      <Input
        id="account-name"
        v-model="form.name"
        :disabled="editing"
        autocomplete="off"
        autocapitalize="none"
        spellcheck="false"
        maxlength="32"
        :placeholder="t('people.namePlaceholder')"
      />

      <template v-if="!editing">
        <Label for="account-password" class="sm:justify-end"
          >{{ t('people.password') }}<span class="text-destructive">*</span></Label
        >
        <div class="flex gap-2">
          <div class="relative flex-1">
            <Input
              id="account-password"
              v-model="form.password"
              :type="showPassword ? 'text' : 'password'"
              autocomplete="new-password"
              class="pr-9 font-mono"
            />
            <button
              type="button"
              class="absolute top-1/2 right-2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
              :aria-label="showPassword ? t('people.hidePassword') : t('people.showPassword')"
              @click="showPassword = !showPassword"
            >
              <EyeOff v-if="showPassword" class="size-4" />
              <Eye v-else class="size-4" />
            </button>
          </div>
          <Button type="button" variant="outline" @click="generate">
            <Dices />
            {{ t('people.generate') }}
          </Button>
        </div>
        <Label for="account-confirm" class="sm:justify-end"
          >{{ t('people.confirm') }}<span class="text-destructive">*</span></Label
        >
        <Input
          id="account-confirm"
          v-model="form.confirm"
          :type="showPassword ? 'text' : 'password'"
          autocomplete="new-password"
          class="font-mono"
        />
        <span class="hidden sm:block" />
        <p class="-mt-1.5 text-xs text-muted-foreground">
          {{ t('people.passwordRules', { min: minLength }) }}
        </p>
      </template>

      <Label for="account-full-name" class="sm:justify-end">{{ t('users.fullName') }}</Label>
      <Input id="account-full-name" v-model="form.fullName" autocomplete="off" maxlength="100" />

      <Label for="account-email" class="sm:justify-end">{{ t('people.email') }}</Label>
      <Input
        id="account-email"
        v-model="form.email"
        type="email"
        autocomplete="off"
        maxlength="254"
      />

      <Label for="account-description" class="sm:justify-end">{{ t('people.description') }}</Label>
      <Input
        id="account-description"
        v-model="form.description"
        autocomplete="off"
        maxlength="200"
      />

      <Label for="account-role" class="sm:justify-end"
        >{{ t('settings.role') }}<span class="text-destructive">*</span></Label
      >
      <div class="grid gap-1">
        <NativeSelect id="account-role" v-model="form.role" :disabled="own">
          <option v-for="option in ROLES" :key="option" :value="option">
            {{ t(`settings.roles.${option}`) }}
          </option>
        </NativeSelect>
        <p class="text-xs text-muted-foreground">{{ t(`users.roleHint.${form.role}`) }}</p>
      </div>

      <Label class="self-start pt-1.5 sm:justify-end">{{ t('people.groups') }}</Label>
      <div class="flex flex-wrap gap-1.5">
        <p v-if="!groups?.length" class="pt-1.5 text-sm text-muted-foreground">
          {{ t('people.noGroups') }}
        </p>
        <label
          v-for="group in groups"
          :key="group.id"
          class="flex cursor-pointer items-center gap-1.5 rounded-full border px-2.5 py-1 text-sm has-[:checked]:border-primary has-[:checked]:bg-primary/10 has-[:checked]:text-primary"
        >
          <input
            type="checkbox"
            class="size-3.5 accent-primary"
            :checked="form.groups.includes(group.id)"
            @change="toggleGroup(group.id, ($event.target as HTMLInputElement).checked)"
          />
          {{ group.name }}
        </label>
      </div>

      <span class="hidden sm:block" />
      <div class="grid gap-1.5 pt-1">
        <div class="flex flex-wrap items-center gap-2">
          <label class="flex items-center gap-2 text-sm" :class="!homesOn && 'opacity-60'">
            <input
              v-model="form.home"
              type="checkbox"
              class="size-4 accent-primary"
              :disabled="!homesOn"
            />
            {{ t('people.homeFolder') }}
          </label>
          <template v-if="form.home && homesOn">
            <Input
              v-model="form.quota"
              type="number"
              min="1"
              step="any"
              class="w-28"
              :placeholder="t('people.unlimited')"
              :aria-label="t('people.quota')"
            />
            <NativeSelect v-model="form.unit" class="w-20" :aria-label="t('people.unit')">
              <option value="GB">GB</option>
              <option value="TB">TB</option>
            </NativeSelect>
          </template>
        </div>
        <p class="text-xs text-muted-foreground">
          {{ homesOn ? t('people.homeFolderHint') : t('people.homesOff') }}
        </p>
        <label class="flex items-center gap-2 text-sm">
          <input
            v-model="form.passwordLocked"
            type="checkbox"
            class="size-4 accent-primary"
            :disabled="own"
          />
          {{ t('people.passwordLocked') }}
        </label>
        <label v-if="editing" class="flex items-center gap-2 text-sm">
          <input
            v-model="form.enabled"
            type="checkbox"
            class="size-4 accent-primary"
            :disabled="own"
          />
          {{ t('users.enabled') }}
        </label>
        <p v-if="own" class="text-xs text-muted-foreground">{{ t('users.ownHint') }}</p>
      </div>
      <button type="submit" class="hidden" />
    </form>

    <SharePermissions v-else v-model="form.rules" :shares="shares ?? []" :effective="effective" />

    <template #footer>
      <Button variant="ghost" :disabled="pending" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button
        v-if="!editing && step === 1"
        variant="secondary"
        :disabled="pending"
        @click="step = 0"
      >
        {{ t('people.back') }}
      </Button>
      <Button v-if="!editing && step === 0" :disabled="!filled" @click="next">{{
        t('people.next')
      }}</Button>
      <Button v-else :disabled="pending || !filled" @click="finish">
        {{ editing ? t('users.save') : t('people.finish') }}
      </Button>
    </template>
  </StepDialog>
</template>
