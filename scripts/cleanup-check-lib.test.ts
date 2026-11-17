// scripts/cleanup-check-lib.ts の単体テスト（mise run test の vitest で実行する）。
import assert from "node:assert/strict";
import { describe, test } from "vitest";
import { cleanupCheck, MAX_SINCE_SUCCESS_MS } from "./cleanup-check-lib.ts";

const NOW = 1_800_000_000_000;
const HOUR = 60 * 60 * 1000;

// SELECT の出力（wrangler d1 execute --json）を作る。
function output(rows: unknown[]): string {
  return JSON.stringify([{ results: rows, success: true }]);
}

describe("cleanupCheck", () => {
  test("3 時間以内に成功していれば ok", () => {
    assert.equal(MAX_SINCE_SUCCESS_MS, 3 * HOUR);
    assert.deepEqual(cleanupCheck(output([{ last_success: NOW - 30 * 60 * 1000 }]), NOW), {
      ok: true,
      message: "自動削除は動いている（最後の成功から 0 時間 30 分）",
    });
    assert.equal(cleanupCheck(output([{ last_success: NOW - MAX_SINCE_SUCCESS_MS }]), NOW)?.ok, true);
  });

  test("3 時間を超えていれば失敗", () => {
    assert.deepEqual(cleanupCheck(output([{ last_success: NOW - 26 * HOUR - 5 * 60 * 1000 }]), NOW), {
      ok: false,
      message: "自動削除が 3 時間 0 分を超えて成功していない（最後の成功から 26 時間 5 分）",
    });
    assert.equal(cleanupCheck(output([{ last_success: NOW - MAX_SINCE_SUCCESS_MS - 1 }]), NOW)?.ok, false);
  });

  test("成功の記録が無ければ失敗（マイグレーションの直後も）", () => {
    assert.deepEqual(cleanupCheck(output([{ last_success: null }]), NOW), {
      ok: false,
      message: "自動削除の成功の記録が無い",
    });
  });

  test("時計がずれて未来の日時でも、負の時間を出さない", () => {
    assert.equal(cleanupCheck(output([{ last_success: NOW + HOUR }]), NOW)?.message, "自動削除は動いている（最後の成功から 0 時間 0 分）");
  });

  test("形が違えば undefined（記録が無いと取り違えない）", () => {
    for (const text of [
      "[]",
      "null",
      "[{}]",
      output([]),
      output([{}]),
      output([null]),
      output([{ last_success: "1" }]),
      output([{ last_success: 1 }, { last_success: 2 }]),
    ]) {
      assert.equal(cleanupCheck(text, NOW), undefined, text);
    }
  });

  test("JSON として読めなければ、例外を投げる", () => {
    assert.throws(() => cleanupCheck("✘ [ERROR] not json", NOW), SyntaxError);
  });
});
