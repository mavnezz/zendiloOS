import { useQuery } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'

/**
 * The shares the signed-in administrator may use, as the file manager has
 * them: where an app or a backup can be put.
 */
export function useShareList() {
  return useQuery({
    queryKey: ['files', 'shares'],
    queryFn: () => unwrap(api.GET('/api/v1/files/shares')),
    select: (shares) =>
      shares
        .filter((share) => !share.home)
        .map((share) => ({
          id: share.id,
          name: share.name,
          writable: share.access === 'read_write',
        }))
        .sort((a, b) => a.name.localeCompare(b.name)),
  })
}

/** The folders in one folder of a share, as the signed-in administrator sees them. */
export async function listFolders(
  share: number,
  path: string,
): Promise<{ names: string[]; writable: boolean }> {
  const folder = await unwrap(
    api.GET('/api/v1/files/shares/{id}/folder', {
      params: { path: { id: share }, query: { path } },
    }),
  )
  const names = folder.entries
    .filter((entry) => entry.kind === 'directory')
    .map((entry) => entry.name)
    .sort((a, b) => a.localeCompare(b))
  return { names, writable: folder.writable }
}

/**
 * The folders in one folder of a share and its files whose name ends in
 * `extension` (`.iso`, in any case), as the signed-in administrator sees them.
 */
export async function listEntries(
  share: number,
  path: string,
  extension: string,
): Promise<{ names: string[]; files: string[]; writable: boolean }> {
  const folder = await unwrap(
    api.GET('/api/v1/files/shares/{id}/folder', {
      params: { path: { id: share }, query: { path } },
    }),
  )
  const sorted = (names: string[]) => names.sort((a, b) => a.localeCompare(b))
  const ending = extension.toLowerCase()
  return {
    names: sorted(folder.entries.filter((e) => e.kind === 'directory').map((e) => e.name)),
    files: sorted(
      folder.entries
        .filter((e) => e.kind === 'file' && e.name.toLowerCase().endsWith(ending))
        .map((e) => e.name),
    ),
    writable: folder.writable,
  }
}

/** Makes the folder `path` in a share. */
export async function makeFolder(share: number, path: string): Promise<void> {
  await unwrap(
    api.POST('/api/v1/files/shares/{id}/folders', {
      params: { path: { id: share } },
      body: { path },
    }),
  )
}
