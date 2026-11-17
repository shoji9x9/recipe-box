// scripts/cleanup-check-lib.ts の単体テスト（mise run test の vitest で実行する）。
import assert from "node:assert/strict";
import { describe, test } from "vitest";
import { checkSql, cleanupCheck, FAILURE_WINDOW_MS, MAX_SINCE_SUCCESS_MS } from "./cleanup-check-lib.ts";

const NOW = 1_800_000_000_000;
const HOUR = 60 * 60 * 1000;

// SELECT の出力（wrangler d1 execute --json）を作る。
function output(rows: unknown[]): string {
  return JSON.stringify([{ results: rows, success: true }]);
}

function row(lastSuccess: number | null, recentFailures = 0): string {
  return output([{ last_success: lastSuccess, recent_failures: recentFailures }]);
}

describe("checkSql", () => {
  test("26 時間前からの失敗を数える", () => {
    assert.equal(FAILURE_WINDOW_MS, 26 * HOUR);
    assert.equal(
      checkSql(NOW),
      "SELECT MAX(CASE WHEN succeeded = TRUE THEN ran_at END) AS last_success, " +
        `COUNT(CASE WHEN succeeded = FALSE AND ran_at >= ${NOW - 26 * HOUR} THEN 1 END) AS recent_failures ` +
        "FROM cleanup_runs",
    );
  });

  test("整数にできない日時は埋め込まない", () => {
    assert.throws(() => checkSql(Number.NaN), RangeError);
  });
});

describe("cleanupCheck", () => {
  test("3 時間以内に成功し、失敗が無ければ ok", () => {
    assert.equal(MAX_SINCE_SUCCESS_MS, 3 * HOUR);
    assert.deepEqual(cleanupCheck(row(NOW - 30 * 60 * 1000), NOW), {
      ok: true,
      message: "自動削除は動いている（最後の成功から 0 時間 30 分）",
    });
    assert.equal(cleanupCheck(row(NOW - MAX_SINCE_SUCCESS_MS), NOW)?.ok, true);
  });

  test("3 時間を超えて成功していなければ失敗", () => {
    assert.deepEqual(cleanupCheck(row(NOW - 26 * HOUR - 5 * 60 * 1000), NOW), {
      ok: false,
      message: "自動削除が 3 時間 0 分を超えて成功していない（最後の成功から 26 時間 5 分）",
    });
    assert.equal(cleanupCheck(row(NOW - MAX_SINCE_SUCCESS_MS - 1), NOW)?.ok, false);
  });

  test("最近に成功していても、失敗した実行があれば失敗（回数は出さない）", () => {
    assert.deepEqual(cleanupCheck(row(NOW - HOUR, 2), NOW), {
      ok: false,
      message: "直近 26 時間 0 分に、成功でない自動削除の実行がある（最後の成功から 1 時間 0 分）",
    });
  });

  test("成功の記録が無ければ失敗（マイグレーションの直後も）", () => {
    assert.deepEqual(cleanupCheck(row(null), NOW), { ok: false, message: "自動削除の成功の記録が無い" });
  });

  test("時計がずれて未来の日時でも、負の時間を出さない", () => {
    assert.equal(cleanupCheck(row(NOW + HOUR), NOW)?.message, "自動削除は動いている（最後の成功から 0 時間 0 分）");
  });

  test("形が違えば undefined（記録が無いと取り違えない）", () => {
    for (const text of [
      "[]",
      "null",
      "[{}]",
      output([]),
      output([{}]),
      output([null]),
      output([{ last_success: NOW }]),
      output([{ recent_failures: 0 }]),
      output([{ last_success: "1", recent_failures: 0 }]),
      output([{ last_success: NOW, recent_failures: "0" }]),
      output([{ last_success: NOW, recent_failures: -1 }]),
      output([{ last_success: NOW, recent_failures: 0.5 }]),
      output([
        { last_success: 1, recent_failures: 0 },
        { last_success: 2, recent_failures: 0 },
      ]),
    ]) {
      assert.equal(cleanupCheck(text, NOW), undefined, text);
    }
  });

  test("JSON として読めなければ、例外を投げる", () => {
    assert.throws(() => cleanupCheck("✘ [ERROR] not json", NOW), SyntaxError);
  });
});
