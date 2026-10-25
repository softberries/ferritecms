import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config.ts";

export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      projects: [
        {
          extends: true,
          test: {
            name: "app",
            environment: "jsdom",
            setupFiles: ["./src/test/setup.ts"],
            include: ["src/**/*.test.{ts,tsx}"],
          },
        },
        {
          extends: true,
          test: {
            name: "scripts",
            environment: "node",
            include: ["scripts/**/*.test.mjs"],
          },
        },
      ],
    },
  }),
);
