<template>
  <el-container style="height: 100vh">
    <el-aside
      width="200px"
      style="border-right: 1px solid #e4e7ed; display: flex; flex-direction: column"
    >
      <div class="logo"><img src="/logo-wordmark.svg" alt="LiteRouter" /></div>
      <el-menu :default-active="$route.path" router class="menu">
        <el-menu-item v-if="isAdmin" index="/channels">渠道管理</el-menu-item>
        <el-menu-item index="/tokens">令牌管理</el-menu-item>
        <el-menu-item v-if="isAdmin" index="/mappings">模型路由</el-menu-item>
        <el-menu-item index="/usage">用量统计</el-menu-item>
        <el-menu-item index="/logs">调用日志</el-menu-item>
        <el-menu-item v-if="isAdmin" index="/users">用户管理</el-menu-item>
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
              <el-dropdown-item command="logout" divided>
                <el-icon><SwitchButton /></el-icon>退出登录
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
import { ArrowRight, SwitchButton } from '@element-plus/icons-vue'
import { logout, me } from '../api'

const router = useRouter()
const username = ref('')
const isAdmin = ref(false)

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
.user-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.chev {
  flex: none;
  margin-left: auto;
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
