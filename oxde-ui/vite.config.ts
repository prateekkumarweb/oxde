import { defineConfig } from "vite-plus";
import path from "node:path";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";

export default defineConfig({
  base: "/dashboard/",
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  plugins: [
    tailwindcss(),
    tanstackRouter({
      target: "react",
      autoCodeSplitting: true,
    }),
    react(),
  ],
  server: {
    proxy: {
      "/api": "http://localhost:3000",
    },
  },
  fmt: {},
  lint: {
    plugins: ["eslint", "typescript", "unicorn", "oxc", "react", "react-perf"],
    jsPlugins: [
      { name: "vite-plus", specifier: "vite-plus/oxlint-plugin" },
      { name: "@tanstack/query", specifier: "@tanstack/eslint-plugin-query" },
    ],
    rules: {
      "vite-plus/prefer-vite-plus-imports": "error",
      "react/react-compiler": "error",
      "@tanstack/query/exhaustive-deps": ["error", { allowlist: { variables: ["api"] } }],
      "@tanstack/query/no-rest-destructuring": "warn",
      "@tanstack/query/stable-query-client": "error",
      "@tanstack/query/no-unstable-deps": "error",
      "@tanstack/query/infinite-query-property-order": "error",
      "@tanstack/query/no-void-query-fn": "error",
      "@tanstack/query/mutation-property-order": "error",
      "@tanstack/query/prefer-query-options": "error",
    },
    options: { typeAware: true, typeCheck: true },
  },
});
