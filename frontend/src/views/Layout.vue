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
        <el-menu-item index="/settings">{{ t('nav.settings') }}</el-menu-item>
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
                <el-tag v-if="isAdmin" size="small" type="success" effect="plain">{{ t('nav.adminBadge') }}</el-tag>
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
import { ArrowRight, SwitchButton } from '@element-plus/icons-vue'
import { logout } from '../api'
import { loadSession, session } from '../session'

const { t } = useI18n()
const router = useRouter()
const username = computed(() => session.username)
const isAdmin = computed(() => session.isAdmin)

// Keep in sync with the `origin` remote / README clone URL.
const repoUrl = 'https://github.com/qihangkong/LiteRouter'

const initial = computed(() => (username.value || '?').charAt(0).toUpperCase())

function onUserCommand(cmd) {
  if (cmd === 'logout') return doLogout()
  if (cmd === 'github') window.open(repoUrl, '_blank', 'noopener,noreferrer')
}

async function doLogout() {
  // finally：登出请求本身失败（会话早已过期）不能把用户困在后台页，
  // 跳转必须发生。
  try {
    await logout()
  } catch (_) {
  } finally {
    router.push('/login')
  }
}

onMounted(loadSession)
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
  /* 头像和用户名靠左，和上方菜单项的文字左边缘对齐；chevron 被
     .user-name 的 flex:1 推到最右。 */
  justify-content: space-between;
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
/* flex:1 让用户名吃掉所有剩余空间，这样 space-between 才真的把 chevron
   推到右边缘；没有它两者会挤在名字右边那一小块里。长名字靠 ellipsis 截断。 */
.user-name {
  flex: 1;
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
</style>
