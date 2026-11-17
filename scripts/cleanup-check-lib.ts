// check-cleanup.ts のうち、wrangler の出力を読んで結果を決める部分（外部に触れないので、単体テストで確かめる）。
import { d1Rows } from "./seed-lib.ts";

/** 最後の成功から、この時間（3 時間）を超えていれば失敗にする。自動削除は 1 時間ごとに動く（wrangler.toml の [triggers]）。 */
export const MAX_SINCE_SUCCESS_MS = 3 * 60 * 60 * 1000;

/** 最後に成功した日時を読む SQL。アプリ全体の 1 つの記録なので、全体を対象にする（AGENTS.md の「SQL」）。 */
export const LAST_SUCCESS_SQL = "SELECT MAX(ran_at) AS last_success FROM cleanup_runs WHERE succeeded = TRUE";

/** 確かめた結果。message は公開のログに出すので、成功か失敗かと、最後の成功から経った時間だけを書く。 */
export type CleanupCheck = { ok: boolean; message: string };

// 経った時間を「N 時間 M 分」で表す（未来の日時は 0 分にする。時計のずれで負にならないように）。
function elapsed(ms: number): string {
  const minutes = Math.floor(Math.max(ms, 0) / 60_000);
  return `${Math.floor(minutes / 60)} 時間 ${minutes % 60} 分`;
}

// LAST_SUCCESS_SQL の出力（wrangler d1 execute --json）と今の時刻から、結果を決める。
// 出力の形が思ったものと違えば undefined を返す（「成功の記録が無い」と取り違えないため）。JSON として読めなければ、その例外を投げる。
export function cleanupCheck(output: string, now: number): CleanupCheck | undefined {
  const rows = d1Rows(output);
  const [row] = rows ?? [];
  if (rows?.length !== 1 || typeof row !== "object" || row === null || !Object.hasOwn(row, "last_success")) {
    return undefined;
  }
  const last: unknown = (row as { last_success: unknown }).last_success;
  if (last === null) {
    return { ok: false, message: "自動削除の成功の記録が無い" };
  }
  if (typeof last !== "number" || !Number.isFinite(last)) {
    return undefined;
  }
  const since = now - last;
  return since <= MAX_SINCE_SUCCESS_MS
    ? { ok: true, message: `自動削除は動いている（最後の成功から ${elapsed(since)}）` }
    : { ok: false, message: `自動削除が ${elapsed(MAX_SINCE_SUCCESS_MS)}を超えて成功していない（最後の成功から ${elapsed(since)}）` };
}
