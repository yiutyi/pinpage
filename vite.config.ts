// 功能：配置 Vue 构建、i18n 资源预编译与 Tauri 使用的固定开发端口。
import { fileURLToPath, URL } from 'node:url';
import VueI18nPlugin from '@intlify/unplugin-vue-i18n/vite';
import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

export default defineConfig({
  plugins: [
    // 资源在构建期编译成消息函数，生产包不内嵌消息编译器，管理界面 CSP 无需放宽。
    VueI18nPlugin({
      include: [fileURLToPath(new URL('./locales/**', import.meta.url))],
      runtimeOnly: true,
      dropMessageCompiler: true,
    }),
    vue(),
  ],
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**', '**/runtime-data/**'] },
  },
  build: {
    target: 'es2022',
    // 管理界面与工具条是独立 WebView，分别声明入口，避免构建时互相覆盖。
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL('./index.html', import.meta.url)),
        toolbar: fileURLToPath(new URL('./toolbar.html', import.meta.url)),
      },
    },
  },
});
