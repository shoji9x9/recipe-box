// JavaScript の単体テストの設定（mise run test）。テストは TypeScript で書く（docs/development.md の「テスト」）。
// - dom: 画面の JavaScript（apps/*/js-tests/）。ブラウザの代わりに happy-dom の DOM で動かす。
// - worker: Worker の中で動く JavaScript（apps/*/js-tests/worker/）。DOM の無い Node のまま動かす（ストリームは Node のもの）。
// - node: Node のスクリプト（scripts/）。Node のまま動かす。
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // テストの間で、fetch などの差し替えを残さない。
    restoreMocks: true,
    unstubGlobals: true,
    projects: [
      {
        extends: true,
        test: {
          name: "dom",
          environment: "happy-dom",
          include: ["apps/*/js-tests/**/*.test.ts"],
          exclude: ["apps/*/js-tests/worker/**"],
        },
      },
      { extends: true, test: { name: "worker", environment: "node", include: ["apps/*/js-tests/worker/**/*.test.ts"] } },
      { extends: true, test: { name: "node", environment: "node", include: ["scripts/**/*.test.ts"] } },
    ],
  },
});
