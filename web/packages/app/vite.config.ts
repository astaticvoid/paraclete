// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "vite";

export default defineConfig({
  esbuild: {
    jsx: "automatic",
    jsxImportSource: "preact",
  },
  build: {
    outDir: "dist",
    target: "es2020",
  },
  // Served from an arbitrary path by tiny_http / the embedded bundle — keep
  // all asset URLs relative so the client works whether the app is at
  // http://host:port/ or (embedded) any mount point.
  base: "./",
});
