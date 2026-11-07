import { keepPreviousData, useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref, ref } from 'vue'

import { ApiProblem, api, csrfToken, unwrap } from '@/shared/api/client'
import type { Favourite, Folder, Job, TransferRequest } from '@/shared/api/types'

import { isDrive, type Place } from './lib/location'

export { joinPath } from './lib/location'

/** Where the file routes of `place` start. */
function base(place: Place): string {
  return isDrive(place)
    ? `/api/v1/files/removable/${encodeURIComponent(place)}`
    : `/api/v1/files/shares/${place}`
}

/** A transfer's target, a share or a drive. */
export function targetOf(place: Place): Pick<TransferRequest, 'target_share' | 'target_drive'> {
  return isDrive(place) ? { target_drive: place } : { target_share: place }
}

/** One folder of a place, or of a share's snapshot. */
export function listFolder(
  place: Place,
  path: string,
  snapshot: string | null = null,
): Promise<Folder> {
  const query = { path, ...(snapshot ? { snapshot } : {}) }
  return isDrive(place)
    ? unwrap(
        api.GET('/api/v1/files/removable/{drive}/folder', {
          params: { path: { drive: place }, query },
        }),
      )
    : unwrap(
        api.GET('/api/v1/files/shares/{id}/folder', { params: { path: { id: place }, query } }),
      )
}

/** How often the drives are looked at while the explorer is shown. */
export const REMOVABLE_POLL_MS = 4000

/**
 * The removable drives plugged in, for someone who manages storage: looked
 * at every few seconds while the explorer is open and shown, so a drive
 * plugged in appears (and is mounted) by itself.
 */
export function useRemovableDrives(enabled: Ref<boolean>) {
  return useQuery({
    queryKey: ['storage', 'removable'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/removable')),
    enabled,
    refetchInterval: REMOVABLE_POLL_MS,
    refetchIntervalInBackground: false,
  })
}

/** Ejecting and mounting drives; the list follows. */
export function useDriveActions() {
  const client = useQueryClient()
  const refreshed = async <T>(request: Promise<T>): Promise<T> => {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['storage', 'removable'] })
    }
  }
  return {
    /** Unmounts it and takes it off the bus, ready to be pulled out. */
    eject: (id: string): Promise<unknown> =>
      refreshed(
        unwrap(
          api.POST('/api/v1/storage/removable/{id}/eject', {
            params: { path: { id } },
            body: { power_off: true },
          }),
        ),
      ),
    mount: (id: string): Promise<unknown> =>
      refreshed(
        unwrap(api.POST('/api/v1/storage/removable/{id}/mount', { params: { path: { id } } })),
      ),
  }
}

/**
 * The volumes and the folders on them the signed-in user sees: all of them
 * for someone who manages shares, otherwise those open to them.
 */
export function useFileVolumes() {
  return useQuery({
    queryKey: ['files', 'volumes'],
    queryFn: () => unwrap(api.GET('/api/v1/files/volumes')),
  })
}

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

/** One folder of a place, or of a share's snapshot; `share` null shows nothing. */
export function useFolder(
  share: Ref<Place | null>,
  path: Ref<string>,
  snapshot: Ref<string | null> = ref(null),
) {
  return useQuery({
    queryKey: ['files', 'folder', share, path, snapshot],
    queryFn: () => listFolder(share.value ?? 0, path.value, snapshot.value),
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
export function downloadUrl(share: Place, path: string, snapshot?: string | null): string {
  return `${base(share)}/download?${query(path, snapshot)}`
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
export function previewUrl(share: Place, path: string, snapshot?: string | null): string {
  return `${base(share)}/download?${query(path, snapshot, { inline: 'true' })}`
}

/** At most this much of a text file is shown. */
export const TEXT_PREVIEW_BYTES = 256 * 1024

/** The beginning of a text file, and whether there is more. */
export async function textPreview(url: string): Promise<{ text: string; more: boolean }> {
  const response = await fetch(url, { headers: { Range: `bytes=0-${TEXT_PREVIEW_BYTES - 1}` } })
  const range = response.headers.get('Content-Range')
  // No byte of an empty file can be asked for: the server says so (416).
  if (response.status === 416 && range === 'bytes */0') return { text: '', more: false }
  if (!response.ok) throw new ApiProblem(response.status, 'unknown')
  const total = range ? Number(range.split('/')[1]) : null
  const text = new TextDecoder().decode(await response.arrayBuffer())
  return { text, more: total !== null && total > TEXT_PREVIEW_BYTES }
}

/** Where the browser downloads a folder from, as a tar archive. */
export function archiveUrl(share: Place, path: string, snapshot?: string | null): string {
  return `${base(share)}/archive?${query(path, snapshot)}`
}

function problem(request: XMLHttpRequest): ApiProblem {
  try {
    const body = JSON.parse(request.responseText) as { code?: string; detail?: string }
    return new ApiProblem(request.status, body.code ?? 'unknown', body.detail)
  } catch {
    return new ApiProblem(request.status, 'unknown')
  }
}

/** How often a running upload tells the server the person is still there. */
export const KEEP_ALIVE_MS = 5 * 60 * 1000

/**
 * Sends `file` as is. Through XMLHttpRequest, because fetch cannot report
 * how much of a request body went out.
 */
export function uploadFile(
  share: Place,
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
    request.open('POST', `${base(share)}/upload?${query}`)
    const token = csrfToken()
    if (token) request.setRequestHeader('X-CSRF-Token', token)
    request.setRequestHeader('Content-Type', 'application/octet-stream')
    request.upload.onprogress = (event) => {
      if (event.lengthComputable && event.total > 0) options.onProgress(event.loaded / event.total)
    }
    // A long upload is the person's doing: it keeps the session alive,
    // which the upload's own request touches only when it starts.
    const alive = setInterval(() => {
      void fetch('/api/v1/auth/session', { credentials: 'same-origin' }).catch(() => undefined)
    }, KEEP_ALIVE_MS)
    const done = (settle: () => void) => () => {
      clearInterval(alive)
      settle()
    }
    request.onload = done(() => (request.status < 300 ? resolve() : reject(problem(request))))
    request.onerror = done(() => reject(new ApiProblem(0, 'network')))
    request.onabort = done(() => reject(new ApiProblem(0, 'aborted')))
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
  const on = (drive: string) => ({ params: { path: { drive } } })
  return {
    refresh,
    createFolder: (share: Place, path: string): Promise<unknown> =>
      refreshing(
        unwrap(
          isDrive(share)
            ? api.POST('/api/v1/files/removable/{drive}/folders', { ...on(share), body: { path } })
            : api.POST('/api/v1/files/shares/{id}/folders', { ...at(share), body: { path } }),
        ),
      ),
    rename: (share: Place, path: string, name: string): Promise<unknown> =>
      refreshing(
        unwrap(
          isDrive(share)
            ? api.POST('/api/v1/files/removable/{drive}/rename', {
                ...on(share),
                body: { path, name },
              })
            : api.POST('/api/v1/files/shares/{id}/rename', { ...at(share), body: { path, name } }),
        ),
      ),
    transfer: (share: Place, body: TransferRequest): Promise<Job> =>
      unwrap(
        isDrive(share)
          ? api.POST('/api/v1/files/removable/{drive}/transfer', { ...on(share), body })
          : api.POST('/api/v1/files/shares/{id}/transfer', { ...at(share), body }),
      ),
    /** Into the person's bin; on a drive, for good. */
    remove: (share: Place, paths: string[]): Promise<Job> =>
      unwrap(
        isDrive(share)
          ? api.POST('/api/v1/files/removable/{drive}/delete', { ...on(share), body: { paths } })
          : api.POST('/api/v1/files/shares/{id}/delete', { ...at(share), body: { paths } }),
      ),
    /** Out of the user's recycle bin; answers where each went. */
    restore: (share: number, paths: string[]): Promise<string[]> =>
      refreshing(
        unwrap(api.POST('/api/v1/files/shares/{id}/restore', { ...at(share), body: { paths } })),
      ).then((restored) => restored.paths),
  }
}
