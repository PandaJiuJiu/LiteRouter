import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  server: {
    host: true, // 监听所有网卡，局域网可通过 http://<本机IP>:5173 访问
    port: 5173,
    proxy: {
      '/api': `http://localhost:${process.env.LITEROUTER_PORT || 3000}`,
      '/v1': `http://localhost:${process.env.LITEROUTER_PORT || 3000}`,
    },
  },
})
