/**
 * 复制文本到剪贴板，兼容两类常见坑：
 *
 * 1. **非安全上下文**（如内网 HTTP 部署 `http://10.4.125.53:5180`）：
 *    `navigator.clipboard.writeText` 仅在安全上下文（HTTPS / localhost）可用，
 *    内网 HTTP + 非 localhost 时 `navigator.clipboard` 为 `undefined`，直接调用会抛错。
 *
 * 2. **Dialog 焦点陷阱**（Radix Dialog 等）：常规兜底「`<textarea>.focus()` + `select()`」
 *    会被焦点陷阱把焦点抢回 Dialog，textarea 失焦后 `execCommand('copy')` 虽返回 true，
 *    实际复制内容为空（表现为「提示复制成功但粘贴为空」）。
 *
 * 因此兜底改为：创建离屏 `<span>` + `Range` 选中（**不依赖焦点**，绕开焦点陷阱），
 * 并监听 `copy` 事件用 `clipboardData.setData` 显式写入内容——即使焦点被抢也能保证
 * 内容写入剪贴板。`white-space: pre` 保留换行。
 *
 * @returns 是否复制成功；调用方据此弹 toast，失败时提示用户手动复制。
 */
export async function copyText(value: string): Promise<boolean> {
  if (!value) return false

  // 优先 Clipboard API（安全上下文）
  if (navigator.clipboard && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(value)
      return true
    } catch {
      // 落到 execCommand 兜底
    }
  }

  return legacyCopy(value)
}

function legacyCopy(value: string): boolean {
  let succeeded = false

  // 监听一次 copy 事件，显式写入内容（焦点陷阱下仍生效，是保证内容的根本）
  const onCopy = (event: ClipboardEvent) => {
    event.preventDefault()
    event.clipboardData?.setData("text/plain", value)
    succeeded = true
  }
  document.addEventListener("copy", onCopy)

  // 离屏 <span> + Range 选中：不依赖焦点，绕开 Dialog 焦点陷阱
  const mark = document.createElement("span")
  mark.textContent = value
  mark.style.position = "fixed"
  mark.style.top = "0"
  mark.style.left = "0"
  mark.style.width = "1px"
  mark.style.height = "1px"
  mark.style.padding = "0"
  mark.style.margin = "-1px"
  mark.style.border = "0"
  mark.style.overflow = "hidden"
  mark.style.whiteSpace = "pre" // 保留换行（测试结果摘要为多行）
  mark.style.userSelect = "text"
  mark.setAttribute("aria-hidden", "true")
  document.body.appendChild(mark)

  const selection = window.getSelection()
  const savedRanges: Range[] = []
  if (selection) {
    for (let i = 0; i < selection.rangeCount; i += 1) {
      savedRanges.push(selection.getRangeAt(i).cloneRange())
    }
    selection.removeAllRanges()
    const range = document.createRange()
    range.selectNodeContents(mark)
    selection.addRange(range)
  }

  try {
    document.execCommand("copy")
  } catch {
    succeeded = false
  } finally {
    if (selection) {
      selection.removeAllRanges()
      savedRanges.forEach((range) => selection.addRange(range))
    }
    document.removeEventListener("copy", onCopy)
    document.body.removeChild(mark)
  }

  return succeeded
}
