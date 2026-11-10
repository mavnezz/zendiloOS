import {
  File,
  FileArchive,
  FileAudio,
  FileCode,
  FileImage,
  FileSpreadsheet,
  FileText,
  FileVideo,
  Folder,
  FolderLock,
  House,
  Link,
  Network,
  Presentation,
} from 'lucide-vue-next'
import type { Component } from 'vue'

import type { Item } from './actions'

/** How an entry looks: its icon and the classes that colour it. */
export interface FileIcon {
  icon: Component
  tone: string
}

/** A plain folder: amber, filled. */
export const FOLDER_TONE =
  'fill-amber-400 text-amber-500 dark:fill-amber-400/85 dark:text-amber-400'
/** A folder drawn with a sign inside it (a lock, a network): lightly filled, so the sign shows. */
export const MARKED_FOLDER_TONE = 'fill-amber-400/30 text-amber-500 dark:text-amber-400'
const MUTED = 'text-muted-foreground'

export type FileFamily =
  | 'image'
  | 'video'
  | 'audio'
  | 'archive'
  | 'pdf'
  | 'document'
  | 'spreadsheet'
  | 'presentation'
  | 'code'

const FAMILIES: Record<FileFamily, string[]> = {
  image: [
    'jpg',
    'jpeg',
    'png',
    'gif',
    'webp',
    'avif',
    'bmp',
    'tif',
    'tiff',
    'svg',
    'heic',
    'heif',
    'ico',
    'psd',
    'raw',
    'dng',
    'cr2',
    'nef',
    'arw',
  ],
  video: ['mp4', 'm4v', 'mkv', 'mov', 'avi', 'wmv', 'webm', 'flv', 'mpg', 'mpeg', 'm2ts', '3gp'],
  audio: ['mp3', 'm4a', 'ogg', 'oga', 'opus', 'flac', 'wav', 'aac', 'wma', 'aiff', 'mid', 'midi'],
  archive: ['zip', 'tar', 'gz', 'tgz', 'bz2', 'xz', 'zst', '7z', 'rar', 'iso', 'dmg', 'deb', 'rpm'],
  pdf: ['pdf'],
  document: ['txt', 'md', 'log', 'rtf', 'doc', 'docx', 'odt', 'pages', 'tex', 'epub'],
  spreadsheet: ['xls', 'xlsx', 'ods', 'csv', 'tsv', 'numbers'],
  presentation: ['ppt', 'pptx', 'odp', 'key'],
  code: [
    'js',
    'mjs',
    'cjs',
    'ts',
    'jsx',
    'tsx',
    'vue',
    'py',
    'rb',
    'go',
    'rs',
    'java',
    'kt',
    'c',
    'h',
    'cpp',
    'hpp',
    'cs',
    'php',
    'sh',
    'bash',
    'zsh',
    'ps1',
    'bat',
    'json',
    'yaml',
    'yml',
    'toml',
    'ini',
    'conf',
    'cfg',
    'env',
    'xml',
    'html',
    'htm',
    'css',
    'scss',
    'sql',
    'nix',
  ],
}

const LOOKS: Record<FileFamily, FileIcon> = {
  image: { icon: FileImage, tone: 'text-violet-500 dark:text-violet-400' },
  video: { icon: FileVideo, tone: 'text-fuchsia-500 dark:text-fuchsia-400' },
  audio: { icon: FileAudio, tone: 'text-rose-500 dark:text-rose-400' },
  archive: { icon: FileArchive, tone: 'text-yellow-700 dark:text-yellow-500' },
  pdf: { icon: FileText, tone: 'text-red-600 dark:text-red-400' },
  document: { icon: FileText, tone: 'text-blue-500 dark:text-blue-400' },
  spreadsheet: { icon: FileSpreadsheet, tone: 'text-green-600 dark:text-green-500' },
  presentation: { icon: Presentation, tone: 'text-orange-500 dark:text-orange-400' },
  code: { icon: FileCode, tone: 'text-cyan-600 dark:text-cyan-400' },
}

/** Each extension, lower case, to the family it belongs to. */
const BY_EXTENSION = new Map<string, FileFamily>(
  (Object.entries(FAMILIES) as [FileFamily, string[]][]).flatMap(([family, extensions]) =>
    extensions.map((ext) => [ext, family] as const),
  ),
)

/** What kind of file `name` is by its extension, if one the explorer tells apart. */
export function fileFamily(name: string): FileFamily | null {
  const dot = name.lastIndexOf('.')
  if (dot <= 0 || dot === name.length - 1) return null
  return BY_EXTENSION.get(name.slice(dot + 1).toLowerCase()) ?? null
}

/**
 * The icon of an entry, in both views: folders amber, files coloured by
 * what they hold; the person's own personal folder, folders that are
 * private or shared on the network, and links keep their own.
 */
export function fileIcon(
  item: Pick<Item, 'name' | 'kind' | 'personal' | 'volumeFolder'>,
): FileIcon {
  if (item.personal?.own) return { icon: House, tone: 'text-sky-500' }
  if (item.personal || item.volumeFolder?.access === null) {
    return { icon: FolderLock, tone: MARKED_FOLDER_TONE }
  }
  if (item.volumeFolder?.shared) return { icon: Network, tone: MARKED_FOLDER_TONE }
  if (item.kind === 'directory') return { icon: Folder, tone: FOLDER_TONE }
  if (item.kind === 'symlink') return { icon: Link, tone: MUTED }
  const family = item.kind === 'file' ? fileFamily(item.name) : null
  return family ? LOOKS[family] : { icon: File, tone: MUTED }
}
