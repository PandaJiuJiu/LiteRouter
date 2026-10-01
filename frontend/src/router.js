import { createRouter, createWebHistory } from 'vue-router'
import { setupStatus } from './api'

const routes = [
  { path: '/setup', component: () => import('./views/Setup.vue') },
  { path: '/login', component: () => import('./views/Login.vue') },
  {
    path: '/',
    component: () => import('./views/Layout.vue'),
    children: [
      { path: '', redirect: '/channels' },
      { path: 'channels', component: () => import('./views/Channels.vue') },
      { path: 'models', component: () => import('./views/Models.vue') },
      { path: 'usage', component: () => import('./views/Usage.vue') },
      { path: 'tokens', component: () => import('./views/Tokens.vue') },
      { path: 'mappings', component: () => import('./views/Mappings.vue') },
      { path: 'logs', component: () => import('./views/Logs.vue') },
      { path: 'logs/:id', component: () => import('./views/LogDetail.vue') },
      { path: 'users', component: () => import('./views/Users.vue') },
      { path: 'settings', component: () => import('./views/Settings.vue') },
    ],
  },
]

const router = createRouter({ history: createWebHistory(), routes })

// Single preflight per navigation: figure out setup state + auth state, then
// decide where to send the user. Login + setup pages are reachable while
// signed out; everything else requires a session.
router.beforeEach(async (to) => {
  const { needsSetup, authenticated } = await setupStatus()

  if (needsSetup) {
    return to.path === '/setup' ? true : '/setup'
  }
  if (to.path === '/setup') {
    // already initialized — setup page is meaningless
    return authenticated ? '/' : '/login'
  }
  if (to.path === '/login') {
    return authenticated ? '/' : true
  }
  if (!authenticated) return '/login'
  return true
})

export default router