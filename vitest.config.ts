// 画面の JavaScript の単体テストの設定（mise run test）。ブラウザの代わりに happy-dom の DOM で動かす。
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "happy-dom",
    include: ["apps/*/js-tests/**/*.test.ts"],
    // テストの間で、fetch などの差し替えを残さない。
    restoreMocks: true,
    unstubGlobals: true,
  },
});
