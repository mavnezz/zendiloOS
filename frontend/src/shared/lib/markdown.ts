import MarkdownIt from 'markdown-it'

/**
 * Markdown as release notes are written: headings, lists, emphasis, code
 * and links. HTML in the source is escaped and shows as text; images are
 * not loaded; links lead only to the web or to e-mail, in a new tab.
 */
const markdown = new MarkdownIt({ html: false, linkify: false, typographer: false }).disable(
  'image',
)

/** Anything else (`javascript:`, `data:`, paths within the NAS) stays text. */
const LINK = /^(https?:|mailto:)/i
markdown.validateLink = (url) => LINK.test(url)

markdown.renderer.rules.link_open = (tokens, index, options, _env, self) => {
  tokens[index]?.attrSet('target', '_blank')
  tokens[index]?.attrSet('rel', 'noopener noreferrer')
  return self.renderToken(tokens, index, options)
}

/** `source` as HTML that is safe to insert into the page. */
export function renderMarkdown(source: string): string {
  return markdown.render(source)
}
