// check-cleanup.ts のうち、wrangler の出力を読んで結果を決める部分（外部に触れないので、単体テストで確かめる）。
import { d1Rows } from "./seed-lib.ts";

const HOUR_MS = 60 * 60 * 1000;

/** 最後の成功から、この時間（3 時間）を超えていれば失敗にする。自動削除は 1 時間ごとに動く（wrangler.toml の [triggers]）。 */
export const MAX_SINCE_SUCCESS_MS = 3 * HOUR_MS;

/**
 * この時間（26 時間）のうちに、成功でない実行が 1 回でもあれば失敗にする。
 * 確かめは毎日動くので、1 日（24 時間）のすべての実行を見る。GitHub の schedule の遅れを見込んで 2 時間を足す（重なった分は 2 回知らせうる）。
 * 自動削除は、対象を実行ごとにずらして選ぶ（src/workspace.rs の pick_for_run）ので、消すたびに失敗するワークスペースがあっても、
 * それを選ばない実行は成功になる。最後の成功だけを見ると、その失敗を見逃すため、失敗した実行も見る。
 */
export const FAILURE_WINDOW_MS = 26 * HOUR_MS;

/**
 * 最後に成功した日時と、FAILURE_WINDOW_MS のうちの成功でない実行の数を読む SQL。
 * アプリ全体の 1 つの記録なので、全体を対象にする（AGENTS.md の「SQL」）。
 * wrangler d1 execute は値を別に渡せないので、整数にした日時を埋め込む。
 */
export function checkSql(now: number): string {
  const since = Math.trunc(now - FAILURE_WINDOW_MS);
  if (!Number.isSafeInteger(since)) {
    throw new RangeError("日時が整数でない");
  }
  return (
    "SELECT MAX(CASE WHEN succeeded = TRUE THEN ran_at END) AS last_success, " +
    `COUNT(CASE WHEN succeeded = FALSE AND ran_at >= ${since} THEN 1 END) AS recent_failures ` +
    "FROM cleanup_runs"
  );
}

/** 確かめた結果。message は公開のログに出すので、成功か失敗かと、最後の成功から経った時間だけを書く（失敗の回数も出さない）。 */
export type CleanupCheck = { ok: boolean; message: string };

// 経った時間を「N 時間 M 分」で表す（未来の日時は 0 分にする。時計のずれで負にならないように）。
function elapsed(ms: number): string {
  const minutes = Math.floor(Math.max(ms, 0) / 60_000);
  return `${Math.floor(minutes / 60)} 時間 ${minutes % 60} 分`;
}

// checkSql の出力（wrangler d1 execute --json）と今の時刻から、結果を決める。
// 出力の形が思ったものと違えば undefined を返す（「成功の記録が無い」と取り違えないため）。JSON として読めなければ、その例外を投げる。
export function cleanupCheck(output: string, now: number): CleanupCheck | undefined {
  const rows = d1Rows(output);
  const [row] = rows ?? [];
  if (
    rows?.length !== 1 ||
    typeof row !== "object" ||
    row === null ||
    !Object.hasOwn(row, "last_success") ||
    !Object.hasOwn(row, "recent_failures")
  ) {
    return undefined;
  }
  const { last_success: last, recent_failures: failures } = row as { last_success: unknown; recent_failures: unknown };
  if (typeof failures !== "number" || !Number.isInteger(failures) || failures < 0) {
    return undefined;
  }
  if (last !== null && (typeof last !== "number" || !Number.isFinite(last))) {
    return undefined;
  }
  if (last === null) {
    return { ok: false, message: "自動削除の成功の記録が無い" };
  }
  const since = now - last;
  if (since > MAX_SINCE_SUCCESS_MS) {
    return {
      ok: false,
      message: `自動削除が ${elapsed(MAX_SINCE_SUCCESS_MS)}を超えて成功していない（最後の成功から ${elapsed(since)}）`,
    };
  }
  if (failures > 0) {
    return {
      ok: false,
      message: `直近 ${elapsed(FAILURE_WINDOW_MS)}に、成功でない自動削除の実行がある（最後の成功から ${elapsed(since)}）`,
    };
  }
  return { ok: true, message: `自動削除は動いている（最後の成功から ${elapsed(since)}）` };
}
