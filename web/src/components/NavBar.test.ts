// @vitest-environment happy-dom
import { describe, it, expect, beforeEach } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createWebHistory } from 'vue-router';
import NavBar from './NavBar.vue';

describe('NavBar navigation order and links', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it('renders navigation links in order: Dashboard -> 模型管理 -> 可观测性', async () => {
    const router = createRouter({
      history: createWebHistory(),
      routes: [
        { path: '/', redirect: '/dashboard' },
        { path: '/dashboard', component: { template: '<div>Dashboard</div>' } },
        { path: '/governance', component: { template: '<div>Governance</div>' } },
        { path: '/recorder', component: { template: '<div>Recorder</div>' } },
        { path: '/connect', component: { template: '<div>Connect</div>' } },
      ],
    });

    const container = document.createElement('div');
    document.body.appendChild(container);

    const app = createApp(NavBar);
    app.use(router);
    await router.push('/dashboard');
    await router.isReady();
    app.mount(container);
    await nextTick();

    const navLinks = Array.from(container.querySelectorAll('nav a'));
    expect(navLinks.length).toBe(3);

    const linkData = navLinks.map((a) => ({
      text: a.textContent?.trim(),
      href: a.getAttribute('href'),
    }));

    // Verify exact order requirement: Dashboard -> 模型管理 -> 可观测性
    expect(linkData[0]).toEqual({ text: 'Dashboard', href: '/dashboard' });
    expect(linkData[1]).toEqual({ text: '模型管理', href: '/governance' });
    expect(linkData[2]).toEqual({ text: '可观测性', href: '/recorder' });

    // Verify 模型管理 is placed before 可观测性
    const govIndex = navLinks.findIndex((a) => a.textContent?.trim() === '模型管理');
    const recIndex = navLinks.findIndex((a) => a.textContent?.trim() === '可观测性');
    expect(govIndex).toBeGreaterThan(-1);
    expect(recIndex).toBeGreaterThan(-1);
    expect(govIndex).toBeLessThan(recIndex);

    app.unmount();
    document.body.removeChild(container);
  });
});
