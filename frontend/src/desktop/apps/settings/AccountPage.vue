<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import ChangePasswordForm from '@/features/auth/components/ChangePasswordForm.vue'
import { useSessionStore } from '@/features/auth/store'
import { DefinitionList, PageCard, PageHeader } from '@/shared/ui/page'

const { t } = useI18n()
const session = useSessionStore()
</script>

<!-- The signed-in account: who it is, and its password. -->
<template>
  <div v-if="session.info">
    <PageHeader :title="t('myAccount.title')" :description="t('myAccount.description')" />
    <div class="grid max-w-3xl gap-4">
      <PageCard :title="t('myAccount.profile')">
        <DefinitionList
          :items="[
            { label: t('myAccount.name'), value: session.info.account.name },
            { label: t('myAccount.role'), value: t(`settings.roles.${session.info.account.role}`) },
          ]"
        />
      </PageCard>
      <PageCard :title="t('settings.changePassword')" :description="t('myAccount.passwordHint')">
        <p v-if="session.info.account.password_locked" class="text-sm text-muted-foreground">
          {{ t('myAccount.passwordLocked') }}
        </p>
        <ChangePasswordForm v-else />
      </PageCard>
    </div>
  </div>
</template>
