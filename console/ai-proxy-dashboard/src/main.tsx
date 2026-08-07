import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import "@/i18n"
import "./index.css"
import App from "./App.tsx"
import { ThemeProvider } from "@/components/theme-provider"
import { Toaster } from "@/components/ui/toast"

async function bootstrap() {
  if (import.meta.env.DEV) {
    const { default: VConsole } = await import("vconsole")
    new VConsole()
    console.info("[LRS] vConsole enabled for development")
  }

  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <ThemeProvider defaultTheme="system" storageKey="lrs-theme">
        <App />
        <Toaster />
      </ThemeProvider>
    </StrictMode>
  )
}

void bootstrap()
