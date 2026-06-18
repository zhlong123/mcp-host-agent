import { defineConfig } from "vite";
import { resolve } from "path";

export default defineConfig({
  base: "./",
  root: resolve(__dirname, "ui"),
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    outDir: resolve(__dirname, "ui/dist"),
    emptyOutDir: true,
    target: "esnext",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    rollupOptions: {
      input: {
        main: resolve(__dirname, "ui/index.html"),
        stream: resolve(__dirname, "ui/stream.html"),
      },
    },
  },
});
