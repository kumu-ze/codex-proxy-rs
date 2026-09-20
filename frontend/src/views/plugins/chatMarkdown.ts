import DOMPurify from 'dompurify'
import { marked } from 'marked'

// 模型输出只作为文档渲染，不允许脚本、表单、图片请求或命名属性进入插件 DOM。
export function renderChatMarkdown(text: string): string {
  const html = marked.parse(text, { async: false, gfm: true, breaks: true })
  const clean = DOMPurify.sanitize(html, {
    ALLOWED_TAGS: ['p', 'br', 'hr', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'strong', 'em', 'del', 'blockquote', 'ul', 'ol', 'li', 'pre', 'code', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'a'],
    ALLOWED_ATTR: ['href', 'title', 'class'],
  })
  const template = document.createElement('template')
  template.innerHTML = clean
  for (const link of template.content.querySelectorAll('a')) {
    link.setAttribute('target', '_blank')
    link.setAttribute('rel', 'noopener noreferrer')
  }
  return template.innerHTML
}
