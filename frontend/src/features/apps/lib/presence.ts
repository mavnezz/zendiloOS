import { useAppProjects, useInstalled, webUrl } from '../api'

/** What an app of a catalog is on this NAS, and so what the store offers for it. */
export type Presence =
  | { kind: 'install' }
  /** A project made from it runs and answers there. */
  | { kind: 'open'; url: string }
  | { kind: 'installed' }
  /** Installed the earlier way, and a job works on it. */
  | { kind: 'busy' }
  /** Installed the earlier way, and that did not finish: to be removed. */
  | { kind: 'failed' }

/**
 * Whether an app is on the NAS: as a Docker project made from it here, or
 * installed the earlier way (one per id).
 */
export function usePresence() {
  const { data: projects } = useAppProjects()
  const { data: installed } = useInstalled()
  return (app: { source_id: number; id: string }): Presence => {
    const made = (projects.value ?? []).filter(
      (project) => project.source_id === app.source_id && project.id === app.id,
    )
    const answering = made.map((project) => webUrl(project)).find((url) => url !== null)
    if (answering) return { kind: 'open', url: answering }
    if (made.length) return { kind: 'installed' }
    const earlier = installed.value?.find((other) => other.id === app.id)
    if (!earlier) return { kind: 'install' }
    if (earlier.state === 'installing' || earlier.state === 'removing') return { kind: 'busy' }
    if (earlier.state === 'failed') return { kind: 'failed' }
    const url = earlier.run_state === 'stopped' ? null : webUrl(earlier)
    return url ? { kind: 'open', url } : { kind: 'installed' }
  }
}
