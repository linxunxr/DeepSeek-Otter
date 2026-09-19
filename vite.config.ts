import { defineConfig } from "vite";
import { resolve } from "node:path";

// Tauri 壳本地页面的构建配置：index.html（加载页/诊断页）+ control.html（控制中心）。
// clearScreen false / envPrefix 让 Tauri CLI 能在终端里正确展示 Rust 编译输出。
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "chrome105",
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        control: resolve(__dirname, "control.html"),
      },
    },
  },
});
