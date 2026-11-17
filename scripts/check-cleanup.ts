#!/usr/bin/env node
// 定期の自動削除が、3 時間以内に成功し（D1 の cleanup_runs）、消し終えているはずのものが D1 に残っていないかを確かめる（Issue #56）。
// そうでなければ終了コード 1 で止まる（判定は scripts/cleanup-check-lib.ts）。
// .github/workflows/seed-demo.yml の job が毎日動かし、失敗すると GitHub が通知のメールを送る（docs/development.md の「失敗の通知」）。
// Node が型の注釈を取り除いて、そのまま動かす（ビルドしない。docs/development.md の「言語」）。
//
// 使い方: node scripts/check-cleanup.ts (--local | --remote)
//   --local:  ローカルの D1（mise run dev が使うもの）
//   --remote: 本番の D1。wrangler の認証（wrangler login か CLOUDFLARE_API_TOKEN）が要る
// 公開の Actions のログに出るので、成功か失敗かと、最後の成功から経った時間と、残っているものの種類だけを表示する（数や ID は出さない）。
import { checkSql, cleanupCheck } from "./cleanup-check-lib.ts";
import { hasOwnKey } from "./seed-lib.ts";
import { redact, type Target, wrangler } from "./wrangler.ts";

const TARGETS: Record<Target, true> = { "--local": true, "--remote": true };

function fail(message: string): never {
  console.error(`::error::${message}`);
  process.exit(1);
}

const args = process.argv.slice(2);
const [target] = args;
if (args.length !== 1 || !hasOwnKey(TARGETS, target)) {
  console.error("使い方: node scripts/check-cleanup.ts (--local | --remote)");
  process.exit(2);
}

const now = Date.now();
let output: string;
try {
  output = await wrangler(target, "d1", "execute", "DB", "--json", "--command", checkSql(now));
} catch (error) {
  // wrangler の失敗の文は wrangler.ts が伏せている。それ以外の例外（起動の失敗など）も、伏せてから出す。
  fail(`D1 を読めない: ${redact(error instanceof Error ? error.message : error)}`);
}

let result: ReturnType<typeof cleanupCheck>;
try {
  result = cleanupCheck(output, now);
} catch {
  fail("wrangler の d1 execute --json の出力を JSON として読めない");
}
if (result === undefined) {
  fail("wrangler の d1 execute --json の出力の形が、思ったものと違う");
}
if (!result.ok) {
  fail(result.message);
}
console.log(result.message);
