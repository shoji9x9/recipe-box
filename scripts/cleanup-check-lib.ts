// check-cleanup.ts のうち、SQL を作り、wrangler の出力を読んで結果を決める部分（外部に触れないので、単体テストで確かめる）。
import { d1Rows } from "./seed-lib.ts";

const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;

/** 最後の成功から、この時間（3 時間）を超えていれば失敗にする。自動削除は 1 時間ごとに動く（wrangler.toml の [triggers]）。 */
export const MAX_SINCE_SUCCESS_MS = 3 * HOUR_MS;

// 以下の 3 つは src/workspace.rs と src/photo.rs の値に合わせる（scripts/cleanup-check-lib.test.ts が、Rust の文字で確かめる）。
/** 最後に使われてから、自動削除の対象になるまでの時間（UNUSED_DELETE_AFTER_MILLIS。14 日）。 */
export const UNUSED_DELETE_AFTER_MS = 14 * DAY_MS;
/** 書き込みか削除が終わっていない印を付けてから、片付けの対象になるまでの時間（PENDING_CLEANUP_AFTER_MILLIS。1 時間）。 */
export const PENDING_CLEANUP_AFTER_MS = HOUR_MS;
/**
 * 対象になってから、自動削除が消し終えているはずの時間（CLEANUP_OVERDUE_MILLIS。1 日）。
 * アプリ全体のワークスペースと写真の行がすべて同時に対象になっても、1 日の実行で消し終える（src/workspace.rs の const の assert）。
 * 対象を実行ごとにずらして選ぶので、消すたびに失敗するものは、この時間を過ぎても残り、確かめで見つかる。
 */
export const CLEANUP_OVERDUE_MS = DAY_MS;
/**
 * 1 日の数を数えるための記録（photo_uploads・workspace_creations）が、消し終えているはずの古さ（2 日）。
 * 自動削除は毎回、UTC の今日の 0 時より前の記録を消すので、2 日より前の記録（昨日の 0 時より前のもの）は、
 * 昨日の 0 時からの実行（24 回以上）で消えているはずである。
 */
export const RECORDS_OVERDUE_MS = 2 * DAY_MS;

/** 残っていてはいけないものの種類と、ログに出す名前（数は出さない）。 */
const OVERDUE = {
  overdue_workspaces: "使われていないワークスペース",
  overdue_photos: "書き込みか削除の途中で残った写真の行",
  overdue_records: "1 日の数を数えるための古い記録",
} as const;

// 埋め込む日時を整数にする（wrangler d1 execute は値を別に渡せない）。
function millis(value: number): number {
  const result = Math.trunc(value);
  if (!Number.isSafeInteger(result)) {
    throw new RangeError("日時が整数でない");
  }
  return result;
}

/**
 * 最後に成功した日時と、自動削除が消し終えているはずなのに残っているものの数を読む SQL。
 * どれもアプリ全体を対象にする（AGENTS.md の「SQL」の、自動削除のクエリ）。どれも索引で読む
 * （cleanup_runs_by_ran_at・workspaces_by_pinned_last_used_at・photos_by_pending_since・photo_uploads_by_created_at・workspace_creations_by_created_at）。
 * 集計なので、表が空でも 1 行を返す。
 */
export function checkSql(now: number): string {
  const workspacesBefore = millis(now - UNUSED_DELETE_AFTER_MS - CLEANUP_OVERDUE_MS);
  const photosBefore = millis(now - PENDING_CLEANUP_AFTER_MS - CLEANUP_OVERDUE_MS);
  const recordsBefore = millis(now - RECORDS_OVERDUE_MS);
  return [
    "SELECT MAX(ran_at) AS last_success,",
    `(SELECT COUNT(*) FROM workspaces WHERE pinned = FALSE AND last_used_at < ${workspacesBefore}) AS overdue_workspaces,`,
    `(SELECT COUNT(*) FROM photos WHERE pending_since < ${photosBefore}) AS overdue_photos,`,
    `(SELECT COUNT(*) FROM photo_uploads WHERE created_at < ${recordsBefore})`,
    `+ (SELECT COUNT(*) FROM workspace_creations WHERE created_at < ${recordsBefore}) AS overdue_records`,
    "FROM cleanup_runs WHERE succeeded = TRUE",
  ].join(" ");
}

/** 確かめた結果。message は公開のログに出すので、成功か失敗かと、最後の成功から経った時間と、残っているものの種類だけを書く（数や ID は出さない）。 */
export type CleanupCheck = { ok: boolean; message: string };

// 経った時間を「N 時間 M 分」で表す（未来の日時は 0 分にする。時計のずれで負にならないように）。
function elapsed(ms: number): string {
  const minutes = Math.floor(Math.max(ms, 0) / 60_000);
  return `${Math.floor(minutes / 60)} 時間 ${minutes % 60} 分`;
}

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

// checkSql の出力（wrangler d1 execute --json）と今の時刻から、結果を決める。
// 出力の形が思ったものと違えば undefined を返す（「成功の記録が無い」と取り違えないため）。JSON として読めなければ、その例外を投げる。
export function cleanupCheck(output: string, now: number): CleanupCheck | undefined {
  const rows = d1Rows(output);
  const [row] = rows ?? [];
  if (rows?.length !== 1 || typeof row !== "object" || row === null) {
    return undefined;
  }
  const values = row as Record<string, unknown>;
  const keys = ["last_success", ...Object.keys(OVERDUE)];
  if (!keys.every((key) => Object.hasOwn(values, key))) {
    return undefined;
  }
  const last = values["last_success"];
  if (last !== null && (typeof last !== "number" || !Number.isFinite(last))) {
    return undefined;
  }
  const kinds = Object.entries(OVERDUE);
  if (!kinds.every(([key]) => isCount(values[key]))) {
    return undefined;
  }
  const overdue = kinds.filter(([key]) => (values[key] as number) > 0).map(([, name]) => name);
  const since = last === null ? undefined : `（最後の成功から ${elapsed(now - last)}）`;
  const problems = [
    ...(last === null ? ["自動削除の成功の記録が無い"] : []),
    ...(last !== null && now - last > MAX_SINCE_SUCCESS_MS
      ? [`自動削除が ${elapsed(MAX_SINCE_SUCCESS_MS)}を超えて成功していない`]
      : []),
    ...(overdue.length > 0 ? [`自動削除が消し終えているはずのものが残っている: ${overdue.join("・")}`] : []),
  ];
  if (problems.length > 0) {
    return { ok: false, message: `${problems.join("。")}${since ?? ""}` };
  }
  return { ok: true, message: `自動削除は動いている${since ?? ""}` };
}
