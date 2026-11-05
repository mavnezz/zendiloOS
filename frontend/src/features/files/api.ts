import { keepPreviousData, useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'

import { ApiProblem, api, csrfToken, unwrap } from '@/shared/api/client'
import type { Job, TransferRequest } from '@/shared/api/types'

/** Shares the signed-in user may use. */
export function useMyShares() {
  return useQuery({
    queryKey: ['files', 'shares'],
    queryFn: () => unwrap(api.GET('/api/v1/files/shares')),
  })
}

/** One folder of a share; `share` null shows nothing. */
export function useFolder(share: Ref<number | null>, path: Ref<string>) {
  return useQuery({
    queryKey: ['files', 'folder', share, path],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/files/shares/{id}/folder', {
          params: { path: { id: share.value ?? 0 }, query: { path: path.value } },
        }),
      ),
    enabled: computed(() => share.value !== null),
    placeholderData: keepPreviousData,
    retry: false,
  })
}

/** Where the browser downloads a file from; the answer is always an attachment. */
export function downloadUrl(share: number, path: string): string {
  return `/api/v1/files/shares/${share}/download?${new URLSearchParams({ path })}`
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
  }
}
