<script setup lang="ts">
import UsersManager from '@/features/accounts/components/UsersManager.vue'
import { useSessionStore } from '@/features/auth/store'

const session = useSessionStore()
</script>

<template>
  <UsersManager :own-account-id="session.info?.account.id" />
</template>
