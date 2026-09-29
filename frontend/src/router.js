import { createRouter, createWebHistory } from 'vue-router'

const routes = [
  { path: '/login', component: () => import('./views/Login.vue') },
  {
    path: '/',
    component: () => import('./views/Layout.vue'),
    children: [
      { path: '', redirect: '/channels' },
      { path: 'channels', component: () => import('./views/Channels.vue') },
      { path: 'models', component: () => import('./views/Models.vue') },
      { path: 'tokens', component: () => import('./views/Tokens.vue') },
      { path: 'logs', component: () => import('./views/Logs.vue') },
    ],
  },
]

const router = createRouter({ history: createWebHistory(), routes })

router.beforeEach((to) => {
  if (to.path !== '/login' && !localStorage.getItem('session')) {
    return '/login'
  }
})

export default router
