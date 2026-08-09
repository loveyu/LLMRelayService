import Markdown from "react-markdown"
import remarkGfm from "remark-gfm"

/**
 * 安全的 Markdown 渲染（对话气泡文本块用）。
 *
 * 安全性：
 * - 基于 react-markdown + remark-gfm，把 Markdown 解析成 HAST 后直接渲染成 React 节点，
 *   全程不使用 dangerouslySetInnerHTML，也**不**引入 rehype-raw —— 因此源码里的任何
 *   HTML / `<script>` / `on*` 事件属性都不会被当作 HTML 执行，只会按字面量当成文本显示，
 *   天然防 XSS。
 * - 链接只允许 http(s) / mailto / tel 等安全协议：react-markdown 默认会过滤掉 `javascript:`
 *   等危险协议的 href，这里沿用默认行为（未放开 rehype-raw / rehype-sanitize 之外的能力）。
 *
 * 样式：通过 `components` 覆写各元素，贴合 shadcn / tailwind 视觉，不依赖 typography 插件。
 * 注意：组件函数里只解构需要的 props、**不展开 `...props`**，避免把 react-markdown 注入的
 * `node` 透传到 DOM 元素触发 React 警告。
 */
export function MarkdownRenderer({ text }: { text: string }) {
  return (
    <div className="text-sm leading-relaxed [overflow-wrap:anywhere]">
      <Markdown
        remarkPlugins={[remarkGfm]}
        components={{
          h1: ({ children }) => (
            <h1 className="mb-2 mt-3 text-base font-bold first:mt-0">{children}</h1>
          ),
          h2: ({ children }) => (
            <h2 className="mb-2 mt-3 text-[15px] font-bold first:mt-0">{children}</h2>
          ),
          h3: ({ children }) => (
            <h3 className="mb-1.5 mt-2.5 text-sm font-bold first:mt-0">{children}</h3>
          ),
          h4: ({ children }) => (
            <h4 className="mb-1.5 mt-2 text-sm font-semibold first:mt-0">{children}</h4>
          ),
          h5: ({ children }) => (
            <h5 className="mb-1 mt-2 text-sm font-semibold first:mt-0">{children}</h5>
          ),
          h6: ({ children }) => (
            <h6 className="mb-1 mt-2 text-xs font-semibold uppercase first:mt-0">
              {children}
            </h6>
          ),
          p: ({ children }) => <p className="mb-2 last:mb-0">{children}</p>,
          a: ({ href, children }) => (
            <a
              href={href}
              target="_blank"
              rel="noopener noreferrer"
              className="font-medium text-primary underline underline-offset-2 hover:opacity-80"
            >
              {children}
            </a>
          ),
          ul: ({ children }) => (
            <ul className="mb-2 ml-4 list-disc space-y-1 last:mb-0">{children}</ul>
          ),
          ol: ({ children }) => (
            <ol className="mb-2 ml-4 list-decimal space-y-1 last:mb-0">{children}</ol>
          ),
          li: ({ children }) => <li className="pl-1">{children}</li>,
          blockquote: ({ children }) => (
            <blockquote className="my-2 border-l-2 border-border pl-3 text-muted-foreground">
              {children}
            </blockquote>
          ),
          hr: () => <hr className="my-3 border-border" />,
          strong: ({ children }) => <strong className="font-semibold">{children}</strong>,
          // 代码：带 `language-` 类名 或 多行 → 视为代码块（外层 <pre> 已加底色/滚动，
          // 这里不加内联底色）；否则当作内联 code（小圆角底色）。
          code: ({ className, children }) => {
            const raw = String(children ?? "")
            const isBlock = /language-/.test(className || "") || raw.includes("\n")
            if (isBlock) {
              return <code className={`font-mono ${className ?? ""}`}>{children}</code>
            }
            return (
              <code className="rounded bg-foreground/10 px-1 py-0.5 font-mono text-[0.85em]">
                {children}
              </code>
            )
          },
          pre: ({ children }) => (
            <pre className="my-2 overflow-x-auto rounded-md bg-foreground/[0.06] p-2.5 text-xs leading-relaxed dark:bg-foreground/[0.04]">
              {children}
            </pre>
          ),
          table: ({ children }) => (
            <div className="my-2 overflow-x-auto">
              <table className="w-full border-collapse text-xs">{children}</table>
            </div>
          ),
          th: ({ children }) => (
            <th className="border border-border bg-muted/60 px-2 py-1 text-left font-semibold">
              {children}
            </th>
          ),
          td: ({ children }) => (
            <td className="border border-border px-2 py-1 align-top">{children}</td>
          ),
          img: ({ src, alt }) => (
            <img src={typeof src === "string" ? src : undefined} alt={alt} className="max-w-full rounded" />
          ),
        }}
      >
        {text}
      </Markdown>
    </div>
  )
}
