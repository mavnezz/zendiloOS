import { computed, ref } from 'vue'

import type { DockerContainer } from '@/shared/api/types'
import { useJobsStore } from '@/shared/jobs/store'

import { useApplyUpdates, useUpdateContainer } from './api'
import { updatesWithoutAsking } from './lib/navigation'

/**
 * Updating containers to the newest image under their tag: asked first
 * unless this browser was told not to, then run as a job whose progress
 * shows like any other.
 */
export function useContainerUpdates() {
  const jobs = useJobsStore()
  const update = useUpdateContainer()
  const applyAll = useApplyUpdates()
  /** The container asked about; all with `askingAll`. */
  const asking = ref<DockerContainer | null>(null)
  const askingAll = ref(false)
  const failure = ref<unknown>(null)

  /** Names of the containers being updated now. */
  const updating = computed(
    () =>
      new Set(
        jobs.running
          .filter((job) => job.kind === 'containers.update' && job.target)
          .map((job) => job.target!),
      ),
  )

  async function start(container: DockerContainer): Promise<void> {
    failure.value = null
    try {
      jobs.update(await update.mutateAsync(container.id))
    } catch (problem) {
      failure.value = problem
    }
  }

  async function startAll(): Promise<void> {
    failure.value = null
    try {
      const started = await applyAll.mutateAsync()
      if (started.job) jobs.update(started.job)
    } catch (problem) {
      failure.value = problem
    }
  }

  function ask(container: DockerContainer): void {
    if (updatesWithoutAsking()) void start(container)
    else asking.value = container
  }

  function askAll(): void {
    if (updatesWithoutAsking()) void startAll()
    else askingAll.value = true
  }

  return { asking, askingAll, ask, askAll, start, startAll, updating, failure }
}
