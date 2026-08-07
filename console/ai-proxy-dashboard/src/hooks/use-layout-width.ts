import * as React from "react"

const STORAGE_KEY = "layout-full-width"

function getSnapshot(): boolean {
  if (typeof window === "undefined") return false
  return localStorage.getItem(STORAGE_KEY) === "true"
}

const listeners = new Set<() => void>()

function subscribe(callback: () => void) {
  listeners.add(callback)
  return () => {
    listeners.delete(callback)
  }
}

export function useLayoutWidth() {
  const fullWidth = React.useSyncExternalStore(subscribe, getSnapshot, getSnapshot)

  const toggleFullWidth = React.useCallback(() => {
    const next = !getSnapshot()
    localStorage.setItem(STORAGE_KEY, String(next))
    listeners.forEach((cb) => cb())
  }, [])

  return [fullWidth, toggleFullWidth] as const
}
