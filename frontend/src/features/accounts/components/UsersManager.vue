<script setup lang="ts">
import { UserRound, UsersRound } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { AppFrame, type NavGroup } from '@/shared/ui/frame'

import AccountsSection from './AccountsSection.vue'
import GroupsSection from './GroupsSection.vue'

/** The signed-in administrator, who cannot change their own access here. */
defineProps<{ ownAccountId: number | undefined }>()

const { t } = useI18n()
const groups = computed<NavGroup[]>(() => [
  {
    id: 'users',
    items: [
      { id: 'accounts', label: t('users.sections.accounts'), icon: UserRound },
      { id: 'groups', label: t('users.sections.groups'), icon: UsersRound },
    ],
  },
])
const section = ref('accounts')
</script>

<!-- Accounts and groups: valid for this interface and SMB alike. -->
<template>
  <AppFrame v-model:section="section" :groups="groups">
    <AccountsSection v-if="section === 'accounts'" :own-account-id="ownAccountId" />
    <GroupsSection v-else />
  </AppFrame>
</template>
