/**
 * 复制文本到剪贴板，兼容非安全上下文（如内网 HTTP 部署）。
 *
 * `navigator.clipboard.writeText` 仅在安全上下文（HTTPS 或 localhost）下可用；
 * 在 `http://<非 localhost>` 下 `navigator.clipboard` 为 `undefined`，直接调用会抛错，
 * 这正是「复制失败，请手动复制」的根因。因此优先用 Clipboard API，不可用或失败时
 * 回退到 `document.execCommand('copy')`（已废弃但仍普遍可用，且不依赖安全上下文）。
 *
 * @returns 是否复制成功；调用方据此弹 toast，失败时提示用户手动复制。
 */
export async function copyText(value: string): Promise<boolean> {
  if (navigator.clipboard && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(value)
      return true
    } catch {
      // 落到 execCommand 兜底
    }
  }

  try {
    const textarea = document.createElement("textarea")
    textarea.value = value
    textarea.setAttribute("readonly", "")
    textarea.style.position = "fixed"
    textarea.style.top = "0"
    textarea.style.left = "0"
    // 透明 + 不可缩放，避免在移动端唤起键盘 / 闪现输入框
    textarea.style.opacity = "0"
    ;(textarea.style as unknown as Record<string, string>).fontSize = "16px"
    document.body.appendChild(textarea)
    textarea.focus()
    textarea.select()
    const ok = document.execCommand("copy")
    document.body.removeChild(textarea)
    return ok
  } catch {
    return false
  }
}
