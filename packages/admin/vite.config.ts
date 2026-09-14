import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

export default defineConfig({
  base: "/admin/",
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    port: 3100,
    proxy: {
      "/health": process.env.DDB_DEV_PROXY || "http://127.0.0.1:7700",
      "/api": {
        target: process.env.DDB_DEV_PROXY || "http://127.0.0.1:7700",
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    sourcemap: true,
  },
});
