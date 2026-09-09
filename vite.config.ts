import { defineConfig } from "vite";

// Tauri 壳本地页面（加载页/诊断页）的构建配置。
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
  },
});
