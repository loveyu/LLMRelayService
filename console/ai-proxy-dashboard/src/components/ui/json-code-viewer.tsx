import { useMemo } from "react"
import CodeMirror, { EditorView } from "@uiw/react-codemirror"
import { json } from "@codemirror/lang-json"
import { oneDark } from "@codemirror/theme-one-dark"
import { useIsDarkMode } from "@/hooks/use-is-dark-mode"

/**
 * 只读 JSON 代码查看器，基于 CodeMirror 6：
 * - 语法高亮（json 语言 + 主题配色）
 * - 折叠（foldGutter，点击 gutter 或快捷键收起/展开对象与数组）
 * - 行号、括号匹配、当前行高亮
 * - 跟随应用深色/浅色主题（oneDark / light）
 *
 * 用在 PayloadPanel 的「原始」模式，替换原来的只读 Textarea：
 * 长文档可折叠，关键值有配色，便于排查 request/response payload。
 */
const viewerTheme = EditorView.theme({
  "&": { height: "100%", fontSize: "11px" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily:
      "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    lineHeight: "1.5",
  },
})

export function JsonCodeViewer({
  value,
  height = "24rem",
}: {
  value: string
  height?: string
}) {
  const isDark = useIsDarkMode()
  const extensions = useMemo(() => [json(), viewerTheme], [])

  return (
    <div className="overflow-hidden rounded-md border border-border">
      <CodeMirror
        value={value}
        readOnly
        editable={false}
        height={height}
        theme={isDark ? oneDark : "light"}
        extensions={extensions}
        basicSetup={{
          lineNumbers: true,
          foldGutter: true,
          highlightActiveLine: true,
          highlightActiveLineGutter: true,
          bracketMatching: true,
          autocompletion: false,
          closeBrackets: false,
          highlightSelectionMatches: false,
          searchKeymap: false,
        }}
      />
    </div>
  )
}
