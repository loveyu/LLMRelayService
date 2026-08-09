import { useEffect, useState } from "react"

/**
 * 监听 `document.documentElement` 上是否存在 `.dark` class（由 ThemeProvider 写入，
 * 兼容 system / 手动切换 / 快捷键 `d` 三种路径）。返回当前是否为深色模式，
 * 供 CodeMirror 这类不直接消费 CSS 变量的第三方组件按需切换主题。
 */
export function useIsDarkMode(): boolean {
  const [dark, setDark] = useState<boolean>(() =>
    typeof document !== "undefined"
      ? document.documentElement.classList.contains("dark")
      : false,
  )

  useEffect(() => {
    const el = document.documentElement
    const update = () => setDark(el.classList.contains("dark"))
    update()

    const observer = new MutationObserver(update)
    observer.observe(el, { attributes: true, attributeFilter: ["class"] })
    return () => observer.disconnect()
  }, [])

  return dark
}
