<template>
  <el-container style="height: 100vh">
    <el-aside width="200px" style="border-right: 1px solid #e4e7ed">
      <div class="logo">Lite One API</div>
      <el-menu :default-active="$route.path" router>
        <el-menu-item index="/channels">渠道管理</el-menu-item>
        <el-menu-item index="/tokens">令牌管理</el-menu-item>
        <el-menu-item index="/logs">调用日志</el-menu-item>
      </el-menu>
      <div class="logout">
        <el-button text type="danger" @click="logout">退出登录</el-button>
      </div>
    </el-aside>
    <el-main style="background: #f5f7fa">
      <router-view />
    </el-main>
  </el-container>
</template>

<script setup>
import { useRouter } from 'vue-router'

const router = useRouter()
function logout() {
  localStorage.removeItem('session')
  router.push('/login')
}
</script>

<style scoped>
.logo {
  font-weight: bold;
  font-size: 18px;
  padding: 20px 16px;
}
.logout {
  position: absolute;
  bottom: 16px;
  left: 16px;
}
</style>
