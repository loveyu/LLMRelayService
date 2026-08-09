import { useMemo } from "react"
import CodeMirror, { EditorView } from "@uiw/react-codemirror"
import { markdown } from "@codemirror/lang-markdown"
import { oneDark } from "@codemirror/theme-one-dark"
import { useIsDarkMode } from "@/hooks/use-is-dark-mode"

/**
 * 只读 Markdown 源码查看器，基于 CodeMirror 6：
 * - markdown 语法高亮（@codemirror/lang-markdown）
 * - 自动换行（lineWrapping）：prose 长段软换行，移动端不横向截断
 * - 跟随应用深色 / 浅色主题（oneDark / light），与 JsonCodeViewer 一致
 * - 不显示行号 / 折叠 / 当前行高亮（源码是 prose，这些是噪音），按内容自动撑高
 *
 * 用于对话气泡「源码」模式。
 */
const viewerTheme = EditorView.theme({
  "&": { fontSize: "12px" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    lineHeight: "1.5",
  },
  ".cm-content": { padding: "8px 10px" },
})

export function MarkdownCodeViewer({ value }: { value: string }) {
  const isDark = useIsDarkMode()
  const extensions = useMemo(
    () => [markdown(), EditorView.lineWrapping, viewerTheme],
    [],
  )

  return (
    <div className="overflow-hidden rounded-md border border-border">
      <CodeMirror
        value={value}
        readOnly
        editable={false}
        theme={isDark ? oneDark : "light"}
        extensions={extensions}
        basicSetup={{
          lineNumbers: false,
          foldGutter: false,
          highlightActiveLine: false,
          highlightActiveLineGutter: false,
          bracketMatching: false,
          autocompletion: false,
          closeBrackets: false,
          highlightSelectionMatches: false,
          searchKeymap: false,
        }}
      />
    </div>
  )
}
