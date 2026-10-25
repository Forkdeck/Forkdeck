import MarkdownIt from "markdown-it";

const markdown = new MarkdownIt({
  html: false,
  linkify: false,
  typographer: false,
});
markdown.validateLink = (url: string) =>
  /^https?:\/\//i.test(url) && !/[\u0000-\u0020]/.test(url);
// Images can track readers; render their alternative text only.
markdown.renderer.rules.image = (tokens, index) =>
  markdown.utils.escapeHtml(tokens[index].content);
export const renderMarkdown = (text: string): string => markdown.render(text);
