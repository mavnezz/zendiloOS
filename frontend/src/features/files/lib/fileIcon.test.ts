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
import { describe, expect, it } from 'vitest'

import type { Item } from './actions'
import { FOLDER_TONE, fileFamily, fileIcon, MARKED_FOLDER_TONE } from './fileIcon'

function file(name: string, extra: Partial<Item> = {}) {
  return { name, kind: 'file' as const, ...extra }
}

describe('fileIcon', () => {
  it('colours files by what they hold, whatever the case of the extension', () => {
    const cases: [string, unknown, string][] = [
      ['beach.JPG', FileImage, 'violet'],
      ['film.mkv', FileVideo, 'fuchsia'],
      ['song.flac', FileAudio, 'rose'],
      ['backup.tar.gz', FileArchive, 'yellow'],
      ['manual.pdf', FileText, 'red'],
      ['letter.docx', FileText, 'blue'],
      ['notes.txt', FileText, 'blue'],
      ['budget.xlsx', FileSpreadsheet, 'green'],
      ['export.csv', FileSpreadsheet, 'green'],
      ['talk.pptx', Presentation, 'orange'],
      ['compose.yaml', FileCode, 'cyan'],
      ['main.rs', FileCode, 'cyan'],
    ]
    for (const [name, icon, colour] of cases) {
      const found = fileIcon(file(name))
      expect(found.icon, name).toBe(icon)
      expect(found.tone, name).toContain(colour)
    }
  })

  it('leaves unknown files, and names without an extension, plain', () => {
    for (const name of ['data.bin', 'README', '.bashrc', 'trailing.']) {
      expect(fileIcon(file(name))).toEqual({ icon: File, tone: 'text-muted-foreground' })
    }
    expect(fileIcon(file('odd.png', { kind: 'other' })).icon).toBe(File)
  })

  it('draws folders amber and keeps the special icons', () => {
    expect(fileIcon(file('Album', { kind: 'directory' }))).toEqual({
      icon: Folder,
      tone: FOLDER_TONE,
    })
    expect(fileIcon(file('link.jpg', { kind: 'symlink' })).icon).toBe(Link)
    expect(
      fileIcon(file('anna', { kind: 'directory', personal: { own: true, volume: 1 } })),
    ).toEqual({ icon: House, tone: 'text-sky-500' })
    expect(
      fileIcon(file('bob', { kind: 'directory', personal: { own: false, volume: 1 } })),
    ).toEqual({ icon: FolderLock, tone: MARKED_FOLDER_TONE })
    expect(
      fileIcon(file('secret', { kind: 'directory', volumeFolder: { shared: true, access: null } }))
        .icon,
    ).toBe(FolderLock)
    expect(
      fileIcon(
        file('photos', { kind: 'directory', volumeFolder: { shared: true, access: 'read' } }),
      ).icon,
    ).toBe(Network)
    expect(
      fileIcon(
        file('media', { kind: 'directory', volumeFolder: { shared: false, access: 'read' } }),
      ),
    ).toEqual({ icon: Folder, tone: FOLDER_TONE })
  })

  it('tells the family by the last extension only', () => {
    expect(fileFamily('photo.jpg.zip')).toBe('archive')
    expect(fileFamily('archive.zip.txt')).toBe('document')
    expect(fileFamily('noextension')).toBeNull()
  })
})
