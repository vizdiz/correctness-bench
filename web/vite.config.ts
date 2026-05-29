import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// The web calls the control plane via same-origin `/v1/...`; the dev server
// proxies that to the control plane. Target is overridable so the same config
// works locally (localhost) and in docker-compose (service name `control`).
const controlTarget = process.env.CONTROL_PROXY_TARGET || 'http://localhost:8000'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    host: true, // listen on 0.0.0.0 so the container port is reachable
    port: 5173,
    proxy: {
      '/v1': { target: controlTarget, changeOrigin: true },
      '/healthz': { target: controlTarget, changeOrigin: true },
    },
  },
})
