<script setup lang="ts">
import { useSessionStore } from '@/features/auth/store'
import FileExplorer from '@/features/files/components/FileExplorer.vue'

const session = useSessionStore()
</script>

<template>
  <FileExplorer
    :user-name="session.info?.account.name"
    :account-id="session.info?.account.id"
    :can-manage="session.can('shares.manage')"
    :can-manage-storage="session.can('storage.manage')"
  />
</template>
