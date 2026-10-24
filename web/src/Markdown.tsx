import { renderMarkdown } from "./render-markdown";
export function Markdown({ text }: { text: string }) {
  return (
    <div
      class="markdown"
      dangerouslySetInnerHTML={{ __html: renderMarkdown(text) }}
    />
  );
}
