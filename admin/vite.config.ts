import { fileURLToPath, URL } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type ProxyOptions } from "vite";

// The dev server proxies the API and previews to a running `ferrite serve`.
// Rewriting `Origin` keeps the server's CSRF origin check passing (07 §2).
const target = process.env.FERRITE_DEV_URL ?? "http://localhost:8080";
const proxy: ProxyOptions = {
  target,
  changeOrigin: true,
  configure: (p) => {
    p.on("proxyReq", (req) => req.setHeader("origin", target));
  },
};

export default defineConfig({
  base: "/admin/",
  plugins: [tanstackRouter({ target: "react", autoCodeSplitting: true }), react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  build: {
    outDir: "dist",
    assetsDir: "assets",
    assetsInlineLimit: 0,
    sourcemap: false,
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: { "/api": proxy, "/_preview": proxy },
  },
});
