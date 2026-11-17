// Builds the page into ui/index.js and ui/style.css for the addon zip.
import { resolve } from "node:path";
import { defineConfig } from "vite";

const here = import.meta.dirname;

export default defineConfig({
  root: here,
  build: {
    outDir: resolve(here, "dist"),
    emptyOutDir: true,
    target: "es2020",
    cssCodeSplit: false,
    lib: { entry: resolve(here, "src/index.ts"), formats: ["es"], fileName: () => "index.js", cssFileName: "style" },
  },
});
