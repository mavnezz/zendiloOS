<script setup lang="ts">
import { UserRound, UsersRound } from 'lucide-vue-next'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'

import AccountsSection from './AccountsSection.vue'
import GroupsSection from './GroupsSection.vue'

type Section = 'accounts' | 'groups'

/** The signed-in administrator, who cannot change their own access here. */
defineProps<{ ownAccountId: number | undefined }>()

const { t } = useI18n()
const sections = [
  { id: 'accounts', icon: UserRound },
  { id: 'groups', icon: UsersRound },
] as const
const section = ref<Section>('accounts')
</script>

<!-- Accounts and groups: valid for this interface and SMB alike. -->
<template>
  <div class="flex h-full flex-col @xl:flex-row">
    <nav class="flex shrink-0 gap-1 border-b p-2 @xl:w-48 @xl:flex-col @xl:border-r @xl:border-b-0">
      <button
        v-for="item in sections"
        :key="item.id"
        type="button"
        :class="
          cn(
            'flex items-center gap-2 rounded-md px-3 py-2 text-sm',
            section === item.id ? 'bg-accent font-medium' : 'hover:bg-accent/60',
          )
        "
        :aria-current="section === item.id ? 'page' : undefined"
        @click="section = item.id"
      >
        <component :is="item.icon" class="size-4" />
        {{ t(`users.sections.${item.id}`) }}
      </button>
    </nav>
    <div class="min-w-0 flex-1 overflow-auto p-4 @xl:p-6">
      <AccountsSection v-if="section === 'accounts'" :own-account-id="ownAccountId" />
      <GroupsSection v-else />
    </div>
  </div>
</template>
