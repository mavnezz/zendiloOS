import { describe, expect, it } from 'vitest'

import { renderMarkdown } from './markdown'

/** The rendered Markdown, as the page would have it. */
function rendered(source: string): HTMLElement {
  const element = document.createElement('div')
  element.innerHTML = renderMarkdown(source)
  return element
}

describe('release notes in Markdown', () => {
  it('has headings, lists, emphasis and code', () => {
    const notes = rendered(
      [
        "## What's Changed",
        '',
        '- A **file explorer** with *folders* of their own',
        '- `zendiloosd` reads __less__',
        '  - nested',
        '',
        '1. first',
        '2. second',
      ].join('\n'),
    )
    expect(notes.querySelector('h2')?.textContent).toBe("What's Changed")
    expect(notes.querySelectorAll('ul > li')).toHaveLength(3)
    expect(notes.querySelector('strong')?.textContent).toBe('file explorer')
    expect(notes.querySelector('em')?.textContent).toBe('folders')
    expect(notes.querySelector('code')?.textContent).toBe('zendiloosd')
    expect(notes.querySelector('li > ul > li')?.textContent).toBe('nested')
    expect(notes.querySelectorAll('ol > li')).toHaveLength(2)
  })

  it('opens links in a new tab, without telling the page there who sent it', () => {
    const link = rendered(
      'See [the release](https://github.com/mavnezz/zendiloOS/releases).',
    ).querySelector('a')!
    expect(link.getAttribute('href')).toBe('https://github.com/mavnezz/zendiloOS/releases')
    expect(link.getAttribute('target')).toBe('_blank')
    expect(link.getAttribute('rel')).toBe('noopener noreferrer')
    expect(link.textContent).toBe('the release')
    expect(rendered('[mail](mailto:nas@example.org)').querySelector('a')).not.toBeNull()
  })

  it('shows HTML as text instead of rendering it', () => {
    const source =
      '<script>alert(1)</script>\n\n<img src=x onerror="alert(2)">\n\nA <b>bold</b> <a href="https://evil.example">claim</a>'
    const notes = rendered(source)
    expect(notes.querySelector('script, img, b, a')).toBeNull()
    expect(notes.textContent).toContain('<script>alert(1)</script>')
    expect(notes.textContent).toContain('<img src=x onerror="alert(2)">')
    expect(notes.textContent).toContain('A <b>bold</b>')
    expect(renderMarkdown('<b>x</b>')).toContain('&lt;b&gt;x&lt;/b&gt;')
  })

  it('makes no links of scripts, data or bare addresses, and loads no images', () => {
    for (const source of [
      '[click](javascript:alert(1))',
      '[click](JAVASCRIPT:alert(1))',
      '[click](data:text/html,<script>alert(1)</script>)',
      '[here](/api/v1/system/power)',
      '<javascript:alert(1)>',
      'See https://github.com/mavnezz/zendiloOS',
    ]) {
      expect(rendered(source).querySelector('a'), source).toBeNull()
    }
    const image = rendered('![tracker](https://example.org/pixel.png)')
    expect(image.querySelector('img')).toBeNull()
  })

  it('keeps a link title from breaking out of its attribute', () => {
    const link = rendered('[x](https://example.org "a\\" onclick=\\"alert(1)")').querySelector('a')!
    expect(link.getAttribute('onclick')).toBeNull()
    expect(link.getAttribute('title')).toBe('a" onclick="alert(1)')
  })
})
