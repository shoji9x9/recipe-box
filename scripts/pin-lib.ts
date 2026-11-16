// pin-workspace.ts のうち、wrangler の出力を読んで結果を決める部分（外部に触れないので、単体テストで確かめる）。
import { d1Rows } from "./seed-lib.ts";

/**
 * 印を変えた後に読んだ結果。
 * - done: 望んだ値になった
 * - missing: その ID のワークスペースが無い
 * - unchanged: 行はあるが、値が望んだものでない
 */
export type PinOutcome = "done" | "missing" | "unchanged";

// UPDATE の後に続けて読んだ SELECT pinned の出力（wrangler d1 execute --json）から、結果を決める。
// D1（SQLite）は BOOLEAN を 0 と 1 で返す。出力の形が思ったものと違えば undefined を返す
// （「ワークスペースが無い」と取り違えないため）。JSON として読めなければ、その例外を投げる。
export function pinOutcome(pin: boolean, output: string): PinOutcome | undefined {
  const rows = d1Rows(output);
  if (rows === undefined) {
    return undefined;
  }
  const [row] = rows;
  if (row === undefined) {
    return "missing";
  }
  if (typeof row !== "object" || row === null || !Object.hasOwn(row, "pinned")) {
    return undefined;
  }
  return (row as { pinned: unknown }).pinned === (pin ? 1 : 0) ? "done" : "unchanged";
}
