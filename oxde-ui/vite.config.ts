import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import path from "node:path";
import { defineConfig } from "vite-plus";

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
  fmt: {
    sortImports: {
      groups: [
        "type-import",
        ["value-builtin", "value-external"],
        "type-internal",
        "value-internal",
        ["type-parent", "type-sibling", "type-index"],
        ["value-parent", "value-sibling", "value-index"],
        "unknown",
      ],
    },
    sortTailwindcss: {
      stylesheet: "./src/style.css",
      functions: ["clsx", "cn"],
      preserveWhitespace: false,
    },
  },
  lint: {
    plugins: ["eslint", "typescript", "unicorn", "oxc", "react", "react-perf"],
    jsPlugins: [
      { name: "vite-plus", specifier: "vite-plus/oxlint-plugin" },
      { name: "@tanstack/query", specifier: "@tanstack/eslint-plugin-query" },
      "oxlint-tailwindcss",
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
      "react/react-in-jsx-scope": "off",

      // Correctness — catch real bugs
      "tailwindcss/no-conflicting-classes": "error",
      "tailwindcss/no-deprecated-classes": "error",
      "tailwindcss/no-duplicate-classes": "warn",
      "tailwindcss/no-unknown-classes": "error",

      // Modernization — keep classes in current canonical form
      "tailwindcss/enforce-canonical": "warn",
      "tailwindcss/no-unnecessary-arbitrary-value": "warn",

      // Style and consistency
      "tailwindcss/enforce-sort-order": "warn",
      "tailwindcss/consistent-variant-order": "warn",
      "tailwindcss/enforce-consistent-important-position": "warn",
      "tailwindcss/no-unnecessary-whitespace": "warn",
    },
    options: { typeAware: true, typeCheck: true },
    categories: {
      correctness: "error",
      suspicious: "warn",
    },
    settings: {
      tailwindcss: {
        entryPoint: "./src/style.css",
      },
    },
  },
});
