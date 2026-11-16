#!/usr/bin/env node
// ワークスペースに、自動削除と削除から外す印（workspaces.pinned）を付ける・外す。管理用で、公開の API にはしない。
// 共有のデモのワークスペースに付ける（docs/spec.md の「デモ用のデータ」）。
// Node が型の注釈を取り除いて、そのまま動かす（ビルドしない。docs/development.md の「言語」）。
//
// 使い方: node scripts/pin-workspace.ts (--local | --remote) [--unpin] < <ID を書いたファイル>
//   --local:  ローカルの D1（mise run dev が使うもの）
//   --remote: 本番の D1。wrangler の認証（wrangler login か CLOUDFLARE_API_TOKEN）が要る
// デモの ID は公開しないので、引数ではなく標準入力で受け取る（引数は mise の表示やシェルの履歴に残る）。ID は表示しない。
// 例: mise run pin -- --remote < ~/demo-workspace-id
import { pinOutcome } from "./pin-lib.ts";
import { hasOwnKey } from "./seed-lib.ts";
import { ID_PATTERN, readIdFromStdin, redact, type Target, wrangler } from "./wrangler.ts";

const TARGETS: Record<Target, true> = { "--local": true, "--remote": true };

function usage(): never {
  console.error("使い方: node scripts/pin-workspace.ts (--local | --remote) [--unpin] < <ID を書いたファイル>");
  process.exit(2);
}

function fail(message: string): never {
  console.error(message);
  process.exit(1);
}

const args = process.argv.slice(2);
const [target, flag] = args;
if (args.length > 2 || !hasOwnKey(TARGETS, target) || (flag !== undefined && flag !== "--unpin")) {
  usage();
}
const pin = flag === undefined;

const id = readIdFromStdin();
// ID は SQL に埋め込むので、形（16 進数の小文字 32 文字）を必ず確かめる。
if (!ID_PATTERN.test(id)) {
  console.error("ワークスペースの ID の形が正しくない（16 進数の小文字 32 文字）");
  process.exit(2);
}

// wrangler d1 execute は値を別に渡せないので、形を確かめた ID を埋め込む。
// ローカルの D1 は変えた件数を返さないので、続けて同じ ID の印を読み、変わったかを確かめる。
const value = pin ? "TRUE" : "FALSE";
const sql = `UPDATE workspaces SET pinned = ${value} WHERE id = '${id}'; SELECT pinned FROM workspaces WHERE id = '${id}'`;
let output: string;
try {
  output = await wrangler(target, "d1", "execute", "DB", "--json", "--command", sql);
} catch (error) {
  // wrangler の失敗の文は wrangler.ts が伏せている。それ以外の例外（起動の失敗など）も、伏せてから出す。
  fail(redact(error instanceof Error ? error.message : error));
}

let outcome: ReturnType<typeof pinOutcome>;
try {
  outcome = pinOutcome(pin, output);
} catch {
  fail("wrangler の d1 execute --json の出力を JSON として読めない");
}
switch (outcome) {
  case "done":
    console.log(pin ? "印を付けた（自動削除と削除の対象から外した）" : "印を外した");
    break;
  case "missing":
    fail("その ID のワークスペースが無い");
  case "unchanged":
    fail("印を変えられなかった");
  case undefined:
    fail("wrangler の d1 execute --json の出力の形が、思ったものと違う");
}
