import { keepPreviousData, useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref, ref } from 'vue'

import { ApiProblem, api, csrfToken, unwrap } from '@/shared/api/client'
import type { Favourite, Job, TransferRequest } from '@/shared/api/types'

/** Shares the signed-in user may use. */
export function useMyShares() {
  return useQuery({
    queryKey: ['files', 'shares'],
    queryFn: () => unwrap(api.GET('/api/v1/files/shares')),
  })
}

/** The signed-in account's favourite folders. */
export function useFavourites() {
  return useQuery({
    queryKey: ['files', 'favourites'],
    queryFn: () => unwrap(api.GET('/api/v1/files/favourites')),
  })
}

/** Replaces the favourites, in the order given. */
export function useSetFavourites() {
  const client = useQueryClient()
  return async (favourites: Pick<Favourite, 'share_id' | 'path'>[]): Promise<Favourite[]> => {
    const saved = await unwrap(api.PUT('/api/v1/files/favourites', { body: { favourites } }))
    client.setQueryData(['files', 'favourites'], saved)
    return saved
  }
}

/** One folder of a share, or of one of its snapshots; `share` null shows nothing. */
export function useFolder(
  share: Ref<number | null>,
  path: Ref<string>,
  snapshot: Ref<string | null> = ref(null),
) {
  return useQuery({
    queryKey: ['files', 'folder', share, path, snapshot],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/files/shares/{id}/folder', {
          params: {
            path: { id: share.value ?? 0 },
            query: { path: path.value, ...(snapshot.value ? { snapshot: snapshot.value } : {}) },
          },
        }),
      ),
    enabled: computed(() => share.value !== null),
    placeholderData: keepPreviousData,
    retry: false,
  })
}

/** `path`, and the snapshot if one is looked into. */
function query(path: string, snapshot?: string | null, extra: Record<string, string> = {}): string {
  return new URLSearchParams({ path, ...(snapshot ? { snapshot } : {}), ...extra }).toString()
}

/** Where the browser downloads a file from, as an attachment. */
export function downloadUrl(share: number, path: string, snapshot?: string | null): string {
  return `/api/v1/files/shares/${share}/download?${query(path, snapshot)}`
}

/** The share's snapshots, newest first; `share` null asks for none. */
export function useShareSnapshots(share: Ref<number | null>) {
  return useQuery({
    queryKey: ['files', 'snapshots', share],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/files/shares/{id}/snapshots', {
          params: { path: { id: share.value ?? 0 } },
        }),
      ),
    enabled: computed(() => share.value !== null),
  })
}

/** Copies entries out of a snapshot back into the share, as a job. */
export function restoreFromSnapshot(share: number, label: string, paths: string[]): Promise<Job> {
  return unwrap(
    api.POST('/api/v1/files/shares/{id}/snapshots/{label}/restore', {
      params: { path: { id: share, label } },
      body: { paths },
    }),
  )
}

export type PreviewKind = 'image' | 'video' | 'audio' | 'text'

/** The kinds the server shows inline (see its download route); others are only saved. */
const PREVIEWS: Record<string, PreviewKind> = {
  jpg: 'image',
  jpeg: 'image',
  png: 'image',
  gif: 'image',
  webp: 'image',
  avif: 'image',
  mp4: 'video',
  m4v: 'video',
  webm: 'video',
  mp3: 'audio',
  m4a: 'audio',
  ogg: 'audio',
  oga: 'audio',
  opus: 'audio',
  flac: 'audio',
  wav: 'audio',
  txt: 'text',
  log: 'text',
  md: 'text',
  csv: 'text',
}

/** How a file can be shown in the browser, if at all. */
export function previewKind(name: string): PreviewKind | null {
  const dot = name.lastIndexOf('.')
  if (dot <= 0) return null
  return PREVIEWS[name.slice(dot + 1).toLowerCase()] ?? null
}

/** Where the browser shows a file from. */
export function previewUrl(share: number, path: string, snapshot?: string | null): string {
  return `/api/v1/files/shares/${share}/download?${query(path, snapshot, { inline: 'true' })}`
}

/** At most this much of a text file is shown. */
export const TEXT_PREVIEW_BYTES = 256 * 1024

/** The beginning of a text file, and whether there is more. */
export async function textPreview(url: string): Promise<{ text: string; more: boolean }> {
  const response = await fetch(url, { headers: { Range: `bytes=0-${TEXT_PREVIEW_BYTES - 1}` } })
  if (!response.ok) throw new ApiProblem(response.status, 'unknown')
  const range = response.headers.get('Content-Range')
  const total = range ? Number(range.split('/')[1]) : null
  const text = new TextDecoder().decode(await response.arrayBuffer())
  return { text, more: total !== null && total > TEXT_PREVIEW_BYTES }
}

/** Where the browser downloads a folder from, as a tar archive. */
export function archiveUrl(share: number, path: string, snapshot?: string | null): string {
  return `/api/v1/files/shares/${share}/archive?${query(path, snapshot)}`
}

/** `parent/name`, or `name` at the root. */
export function joinPath(parent: string, name: string): string {
  return parent ? `${parent}/${name}` : name
}

function problem(request: XMLHttpRequest): ApiProblem {
  try {
    const body = JSON.parse(request.responseText) as { code?: string; detail?: string }
    return new ApiProblem(request.status, body.code ?? 'unknown', body.detail)
  } catch {
    return new ApiProblem(request.status, 'unknown')
  }
}

/**
 * Sends `file` as is. Through XMLHttpRequest, because fetch cannot report
 * how much of a request body went out.
 */
export function uploadFile(
  share: number,
  folder: string,
  file: File,
  options: { replace: boolean; onProgress: (fraction: number) => void; signal?: AbortSignal },
): Promise<void> {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest()
    const query = new URLSearchParams({
      path: folder,
      name: file.name,
      replace: String(options.replace),
    })
    request.open('POST', `/api/v1/files/shares/${share}/upload?${query}`)
    const token = csrfToken()
    if (token) request.setRequestHeader('X-CSRF-Token', token)
    request.setRequestHeader('Content-Type', 'application/octet-stream')
    request.upload.onprogress = (event) => {
      if (event.lengthComputable && event.total > 0) options.onProgress(event.loaded / event.total)
    }
    request.onload = () => (request.status < 300 ? resolve() : reject(problem(request)))
    request.onerror = () => reject(new ApiProblem(0, 'network'))
    request.onabort = () => reject(new ApiProblem(0, 'aborted'))
    options.signal?.addEventListener('abort', () => request.abort())
    request.send(file)
  })
}

export function useFileActions() {
  const client = useQueryClient()
  const refresh = () => client.invalidateQueries({ queryKey: ['files', 'folder'] })
  const refreshing = async <T>(request: Promise<T>): Promise<T> => {
    try {
      return await request
    } finally {
      await refresh()
    }
  }
  const at = (share: number) => ({ params: { path: { id: share } } })
  return {
    refresh,
    createFolder: (share: number, path: string): Promise<unknown> =>
      refreshing(
        unwrap(api.POST('/api/v1/files/shares/{id}/folders', { ...at(share), body: { path } })),
      ),
    rename: (share: number, path: string, name: string): Promise<unknown> =>
      refreshing(
        unwrap(
          api.POST('/api/v1/files/shares/{id}/rename', { ...at(share), body: { path, name } }),
        ),
      ),
    transfer: (share: number, body: TransferRequest): Promise<Job> =>
      unwrap(api.POST('/api/v1/files/shares/{id}/transfer', { ...at(share), body })),
    remove: (share: number, paths: string[]): Promise<Job> =>
      unwrap(api.POST('/api/v1/files/shares/{id}/delete', { ...at(share), body: { paths } })),
    /** Out of the user's recycle bin; answers where each went. */
    restore: (share: number, paths: string[]): Promise<string[]> =>
      refreshing(
        unwrap(api.POST('/api/v1/files/shares/{id}/restore', { ...at(share), body: { paths } })),
      ).then((restored) => restored.paths),
  }
}
