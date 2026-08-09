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
 * - 自动换行（lineWrapping）：长行/长值软换行，移动端不再被横向截断
 * - 跟随应用深色/浅色主题（oneDark / light）
 * - height 不传时编辑器按内容自动撑高（适合请求/响应头这类短 JSON）；
 *   传了则固定高度、内部滚动（适合大 payload）。
 *
 * 用在 PayloadPanel 的「原始」模式 + 详情页的 headers 卡片。
 */
const viewerTheme = EditorView.theme({
  "&": { fontSize: "11px" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily:
      "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    lineHeight: "1.5",
  },
})

export function JsonCodeViewer({
  value,
  height,
}: {
  value: string
  /** 固定高度（如 "24rem"）；不传则按内容自动撑高。 */
  height?: string
}) {
  const isDark = useIsDarkMode()
  const extensions = useMemo(
    () => [json(), EditorView.lineWrapping, viewerTheme],
    [],
  )

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
