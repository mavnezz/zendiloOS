import type { DockerContainer, DockerProject } from '@/shared/api/types'
import type { BadgeVariants } from '@/shared/ui/badge'

export type Status = 'running' | 'partly' | 'stopped' | 'empty'

export function projectStatus(project: DockerProject): Status {
  const total = project.containers.length
  if (total === 0) return 'empty'
  if (project.running === total) return 'running'
  return project.running === 0 ? 'stopped' : 'partly'
}

export function statusVariant(status: Status): BadgeVariants['variant'] {
  return status === 'running' ? 'success' : status === 'partly' ? 'warning' : 'secondary'
}

/** Its processes exist: as the server counts it. */
export function isUp(container: DockerContainer): boolean {
  return ['running', 'paused', 'restarting'].includes(container.state)
}
