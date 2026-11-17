// scripts/cleanup-check-lib.ts の単体テスト（mise run test の vitest で実行する）。
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, test } from "vitest";
import {
  CLEANUP_OVERDUE_MS,
  checkSql,
  cleanupCheck,
  MAX_SINCE_SUCCESS_MS,
  PENDING_CLEANUP_AFTER_MS,
  UNUSED_DELETE_AFTER_MS,
} from "./cleanup-check-lib.ts";

const NOW = 1_800_000_000_000;
const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;

// SELECT の出力（wrangler d1 execute --json）を作る。
function output(rows: unknown[]): string {
  return JSON.stringify([{ results: rows, success: true }]);
}

function row(lastSuccess: number | null, overdue: Partial<Record<string, unknown>> = {}): string {
  return output([{ last_success: lastSuccess, overdue_workspaces: 0, overdue_photos: 0, ...overdue }]);
}

describe("Rust の値と同じか", () => {
  // Rust の定数を変えたら、ここで気付く（変えたら cleanup-check-lib.ts の値も直す）。
  const source = (file: string) => readFileSync(join(import.meta.dirname, "..", "apps", "recipe-box-rs", "src", file), "utf8");

  test("自動削除の対象になるまでの時間と、消し終える時間", () => {
    const workspace = source("workspace.rs");
    assert.match(workspace, /pub const UNUSED_DELETE_AFTER_MILLIS: i64 = 14 \* DAY_MILLIS;/);
    assert.match(workspace, /pub const CLEANUP_OVERDUE_MILLIS: i64 = DAY_MILLIS;/);
    assert.match(source("photo.rs"), /pub const PENDING_CLEANUP_AFTER_MILLIS: i64 = 60 \* 60 \* 1_000;/);
    assert.equal(UNUSED_DELETE_AFTER_MS, 14 * DAY);
    assert.equal(CLEANUP_OVERDUE_MS, DAY);
    assert.equal(PENDING_CLEANUP_AFTER_MS, HOUR);
  });
});

describe("checkSql", () => {
  test("消し終えているはずの境目を埋め込む", () => {
    assert.equal(
      checkSql(NOW),
      "SELECT MAX(ran_at) AS last_success, " +
        `(SELECT COUNT(*) FROM workspaces WHERE pinned = FALSE AND last_used_at < ${NOW - 15 * DAY}) AS overdue_workspaces, ` +
        `(SELECT COUNT(*) FROM photos WHERE pending_since < ${NOW - DAY - HOUR}) AS overdue_photos ` +
        "FROM cleanup_runs WHERE succeeded = TRUE",
    );
  });

  test("整数にできない日時は埋め込まない", () => {
    assert.throws(() => checkSql(Number.NaN), RangeError);
  });
});

describe("cleanupCheck", () => {
  test("3 時間以内に成功し、残っているものが無ければ ok", () => {
    assert.equal(MAX_SINCE_SUCCESS_MS, 3 * HOUR);
    assert.deepEqual(cleanupCheck(row(NOW - 30 * 60 * 1000), NOW), {
      ok: true,
      message: "自動削除は動いている。最後の成功から 0 時間 30 分",
    });
    assert.equal(cleanupCheck(row(NOW - MAX_SINCE_SUCCESS_MS), NOW)?.ok, true);
  });

  test("3 時間を超えて成功していなければ失敗", () => {
    assert.deepEqual(cleanupCheck(row(NOW - 26 * HOUR - 5 * 60 * 1000), NOW), {
      ok: false,
      message: "自動削除が 3 時間 0 分を超えて成功していない。最後の成功から 26 時間 5 分",
    });
    assert.equal(cleanupCheck(row(NOW - MAX_SINCE_SUCCESS_MS - 1), NOW)?.ok, false);
  });

  test("最近に成功していても、消し終えているはずのものが残っていれば失敗（種類だけを出し、数は出さない）", () => {
    assert.deepEqual(cleanupCheck(row(NOW - HOUR, { overdue_workspaces: 2 }), NOW), {
      ok: false,
      message: "自動削除が消し終えているはずのものが残っている: 使われていないワークスペース。最後の成功から 1 時間 0 分",
    });
    assert.deepEqual(cleanupCheck(row(NOW - 5 * HOUR, { overdue_workspaces: 1, overdue_photos: 3 }), NOW), {
      ok: false,
      message:
        "自動削除が 3 時間 0 分を超えて成功していない。" +
        "自動削除が消し終えているはずのものが残っている: 使われていないワークスペース・書き込みか削除の途中で残った写真の行。" +
        "最後の成功から 5 時間 0 分",
    });
  });

  test("成功の記録が無ければ失敗（マイグレーションの直後も）", () => {
    assert.deepEqual(cleanupCheck(row(null), NOW), { ok: false, message: "自動削除の成功の記録が無い" });
    assert.deepEqual(cleanupCheck(row(null, { overdue_photos: 1 }), NOW), {
      ok: false,
      message: "自動削除の成功の記録が無い。自動削除が消し終えているはずのものが残っている: 書き込みか削除の途中で残った写真の行",
    });
  });

  test("時計がずれて未来の日時でも、負の時間を出さない", () => {
    assert.equal(cleanupCheck(row(NOW + HOUR), NOW)?.message, "自動削除は動いている。最後の成功から 0 時間 0 分");
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
      output([{ last_success: NOW, overdue_workspaces: 0 }]),
      row("1" as unknown as number),
      row(NOW, { overdue_workspaces: "0" }),
      row(NOW, { overdue_photos: -1 }),
      row(NOW, { overdue_photos: 0.5 }),
      row(NOW, { overdue_workspaces: null }),
      output([JSON.parse(row(NOW))[0].results[0], JSON.parse(row(NOW))[0].results[0]]),
    ]) {
      assert.equal(cleanupCheck(text, NOW), undefined, text);
    }
  });

  test("JSON として読めなければ、例外を投げる", () => {
    assert.throws(() => cleanupCheck("✘ [ERROR] not json", NOW), SyntaxError);
  });
});
