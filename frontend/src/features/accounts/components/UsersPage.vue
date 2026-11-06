<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { PageHeader, PageTabs } from '@/shared/ui/page'

import AccountRules from './AccountRules.vue'
import GroupsTable from './GroupsTable.vue'
import UsersTable from './UsersTable.vue'

defineProps<{ ownAccountId: number | undefined }>()

const { t } = useI18n()
const tab = ref('users')
</script>

<!-- Accounts, the groups they are in, and the rules that apply to all of them. -->
<template>
  <div class="flex min-h-full flex-col">
    <PageHeader
      :title="t('controlPanel.accounts')"
      :description="t('controlPanel.accountsDescription')"
    />
    <PageTabs
      v-model="tab"
      :label="t('controlPanel.accounts')"
      :tabs="[
        { id: 'users', label: t('people.tabs.users') },
        { id: 'groups', label: t('people.tabs.groups') },
        { id: 'advanced', label: t('people.tabs.advanced') },
      ]"
    />
    <UsersTable v-if="tab === 'users'" :own-account-id="ownAccountId" />
    <GroupsTable v-else-if="tab === 'groups'" :own-account-id="ownAccountId" />
    <AccountRules v-else />
  </div>
</template>
