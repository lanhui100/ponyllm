import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';
import { router, setToastHandler } from './router';
import { toast } from './composables/useToast';
import './style.css';

// 注册全局 401 单飞重定向的 toast 处理器
setToastHandler((msg) => {
  toast.warning(msg);
});

const app = createApp(App);
app.use(createPinia());
app.use(router);
app.mount('#app');
