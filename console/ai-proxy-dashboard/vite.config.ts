import path from "path"
import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"

// 后端来源 (dev 代理目标)。可用 LRS_BACKEND_ORIGIN 覆盖, 默认 http://127.0.0.1:3300。
const backendOrigin = process.env.LRS_BACKEND_ORIGIN ?? "http://127.0.0.1:3300"

// https://vite.dev/config/
export default defineConfig({
  base: "/",
  plugins: [react(), tailwindcss()],
  build: {
    outDir: path.resolve(__dirname, "../../dist/frontend"),
    assetsDir: "dashboard-assets",
    emptyOutDir: true,
  },
  server: {
    host: "0.0.0.0",
    port: 5173,
    proxy: {
      "/__console": {
        target: backendOrigin,
        changeOrigin: true,
      },
      "/__debug": {
        target: backendOrigin,
        changeOrigin: true,
      },
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
})
