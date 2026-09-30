<template>
  <el-container style="height: 100vh">
    <el-aside
      width="200px"
      style="border-right: 1px solid #e4e7ed; display: flex; flex-direction: column"
    >
      <div class="logo"><img src="/logo-wordmark.svg" alt="LiteRouter" /></div>
      <el-menu :default-active="$route.path" router class="menu">
        <el-menu-item v-if="isAdmin" index="/channels">{{ t('nav.channels') }}</el-menu-item>
        <el-menu-item index="/tokens">{{ t('nav.tokens') }}</el-menu-item>
        <el-menu-item v-if="isAdmin" index="/mappings">{{ t('nav.mappings') }}</el-menu-item>
        <el-menu-item index="/usage">{{ t('nav.usage') }}</el-menu-item>
        <el-menu-item index="/logs">{{ t('nav.logs') }}</el-menu-item>
        <el-menu-item v-if="isAdmin" index="/users">{{ t('nav.users') }}</el-menu-item>
      </el-menu>
      <div class="user-wrap">
        <el-dropdown trigger="click" placement="top-end" @command="onUserCommand">
          <div class="user-box" :title="username">
            <el-avatar :size="28" class="avatar">{{ initial }}</el-avatar>
            <span class="user-name">{{ username }}</span>
            <el-icon class="chev"><ArrowRight /></el-icon>
          </div>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item disabled>
                <span class="dd-user">{{ username }}</span>
                <el-tag v-if="isAdmin" size="small" type="success" effect="plain">admin</el-tag>
              </el-dropdown-item>
              <el-dropdown-item command="github" divided>
                <el-icon>
                  <svg class="repo-icon" viewBox="0 0 16 16" aria-hidden="true">
                    <path
                      fill="currentColor"
                      d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.012 8.012 0 0 0 16 8c0-4.42-3.58-8-8-8Z"
                    />
                  </svg>
                </el-icon>{{ t('nav.github') }}
              </el-dropdown-item>
              <!-- el-dropdown 不支持嵌套下拉，语言选项平铺成两项，
                   当前语言打勾。选项文案恒用各自母语。 -->
              <el-dropdown-item command="lang:zh-CN" divided
                :class="{ 'lang-active': locale === 'zh-CN' }">
                <span>{{ LANGUAGES[0].label }}</span>
                <el-icon v-if="locale === 'zh-CN'" class="lang-check"><Select /></el-icon>
              </el-dropdown-item>
              <el-dropdown-item command="lang:en-US"
                :class="{ 'lang-active': locale === 'en-US' }">
                <span>{{ LANGUAGES[1].label }}</span>
                <el-icon v-if="locale === 'en-US'" class="lang-check"><Select /></el-icon>
              </el-dropdown-item>
              <el-dropdown-item command="logout" divided>
                <el-icon><SwitchButton /></el-icon>{{ t('nav.logout') }}
              </el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </div>
    </el-aside>
    <el-main style="background: #f5f7fa">
      <router-view />
    </el-main>
  </el-container>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { ArrowRight, Select, SwitchButton } from '@element-plus/icons-vue'
import { LANGUAGES, i18n, setLocale } from '../i18n'
import { logout, me, setLanguage } from '../api'

const { t } = useI18n()
const router = useRouter()
const locale = computed(() => i18n.global.locale.value)
const username = ref('')
const isAdmin = ref(false)

// Keep in sync with the `origin` remote / README clone URL.
const repoUrl = 'https://github.com/qihangkong/LiteRouter'

const initial = computed(() => (username.value || '?').charAt(0).toUpperCase())

async function loadMe() {
  try {
    const u = await me()
    username.value = u.username
    isAdmin.value = !!u.is_admin
  } catch (_) {
    // 401 etc — interceptor handles redirect
  }
}

function onUserCommand(cmd) {
  if (cmd === 'logout') return doLogout()
  if (cmd === 'github') window.open(repoUrl, '_blank', 'noopener,noreferrer')
  if (cmd?.startsWith('lang:')) return switchLanguage(cmd.slice(5))
}

// 先本地切换、立刻可见，再写库。语言是纯展示设置，不值得为它转圈；
// 万一写库失败，下次加载会回到 DB 里的旧值，行为可预期。
async function switchLanguage(lang) {
  if (lang === locale.value) return
  setLocale(lang)
  try {
    await setLanguage(lang)
  } catch (_) {
    // 拦截器已经弹过提示了；本地状态保持用户刚选的，不回滚
  }
}

async function doLogout() {
  await logout()
  router.push('/login')
}

onMounted(loadMe)
</script>

<style scoped>
.logo {
  padding: 18px 16px;
  border-bottom: 1px solid #f1f5f9;
}
.logo img {
  display: block;
  width: 100%;
  height: auto;
  max-width: 168px;
}
/* Menu takes the slack so the user bubble always pins to the bottom of the
   flex column. */
.menu {
  flex: 1;
  min-height: 0;
}
/* Inlined because Element Plus ships no GitHub mark. Sits inside el-icon so
   it inherits the menu item's color and baseline. */
.repo-icon {
  width: 1em;
  height: 1em;
  display: block;
}
/* Flat row pinned to the bottom of the sidebar — styled as a peer of the
   el-menu items above so it reads as part of the list, not a separate widget.
   A top rule separates it from the menu. */
.user-wrap {
  border-top: 1px solid #f1f5f9;
}
/* el-dropdown defaults to inline-block so it shrinks to fit its content;
   force it full-width so .user-box fills the sidebar like the menu items. */
:deep(.el-dropdown) {
  display: block;
}
.user-box {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 10px 20px;
  color: #303133;
  font-size: 14px;
  cursor: pointer;
  user-select: none;
  transition: background 0.15s;
}
.user-box:hover {
  background: #f5f7fa;
}
.avatar {
  flex: none;
  background: #dbe4f0;
  color: #3d5a80;
  font-size: 14px;
}
/* No flex:1 here — the row is centered as a group, so the name must size to
   its content and let the chevron sit right beside it. */
.user-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.chev {
  flex: none;
  transform: rotate(-90deg);
  color: #909399;
  font-size: 12px;
}
/* Active menu item renders as a full-width horizontal bar instead of the
   default left-side indicator — easier to scan the sidebar. */
:deep(.el-menu-item.is-active) {
  background: #ecf5ff;
  color: #409eff;
}
:deep(.el-menu-item.is-active::before) {
  background-color: transparent;
}
.dd-user {
  margin-right: 6px;
}
/* Language rows: label left, checkmark right — the check is what tells you
   which one is live now. */
:deep(.el-dropdown-menu__item.lang-active) {
  color: #409eff;
}
.lang-check {
  margin-left: 12px;
  font-size: 12px;
}
</style>
