import { createApp } from 'vue'
import ElementPlus from 'element-plus'
import 'element-plus/dist/index.css'
import './styles.css'
import App from './App.vue'
import router from './router'
import i18n, { setLocale } from './i18n'
import { setupStatus } from './api'

// 语言是全局配置，没有"猜"这一说 —— 向导里选了写库，之后每次加载直接读
// 后端。第一次渲染前就把 locale 定下来，避免中文闪一下再切英文。
// localStorage 先用上：后端不可达时至少首屏语言是对的。
async function bootstrap() {
  let lang = localStorage.getItem('literouter.lang')
  try {
    const status = await setupStatus()
    if (status?.language) lang = status.language
  } catch (_) {
    // 后端还没起来 / 网络不通 —— 沿用 localStorage 或默认值
  }
  setLocale(lang)
  createApp(App).use(ElementPlus).use(i18n).use(router).mount('#app')
}

bootstrap()