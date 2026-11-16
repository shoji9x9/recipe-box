// scripts/wrangler.ts のうち、外部に触れない部分（wrangler の失敗の文を作る）。単体テストは scripts/wrangler-lib.test.ts。
// 失敗の文は公開の Actions のログに出るので、ID とパスを伏せる手順を、ここで確かめられる形にしておく。
import { stripVTControlCharacters } from "node:util";
import { type PathRule, redact } from "./seed-lib.ts";

/** execFile の失敗が持つ値のうち、失敗の文に使うもの。 */
export type WranglerFailure = {
  stdout?: string;
  stderr?: string;
  code?: string | number | null;
  signal?: string | null;
  name?: string;
};

/** 失敗の文に残す出力の長さ。超えたら、頭と終わりを半分ずつ残す。 */
export const FAILURE_OUTPUT_LENGTH = 4000;

// wrangler の失敗から、表示する文を作る。paths は伏せるパス（seed-lib.ts の pathPattern で作る）。
export function wranglerFailureMessage(failure: WranglerFailure, paths: readonly PathRule[]): string {
  // 理由は stderr か stdout（--json のとき）にある。Node のエラーの文は、コマンドの全体（ホームのディレクトリのパスと SQL）を
  // 含むので使わず、終了コードかシグナル（起動できなければ ENOENT などのコード）だけを添える。
  // 制御文字（色など）は、伏せる前に取り除く（制御文字に挟まれた値も伏せるため）。
  // ログのファイルの場所の行は、理由にならないので除く。
  const output = redact(
    stripVTControlCharacters(
      [failure.stderr, failure.stdout]
        .filter((text) => text?.trim())
        .join("\n")
        .split("\n")
        .filter((line) => !line.includes("Logs were written"))
        .join("\n"),
    ),
    paths,
  );
  // 終了コードもシグナルも無ければ（execFile ではふつう起きない）、エラーの名前を添える。
  const detail = [failure.code, failure.signal].filter((value) => value != null).join("・") || failure.name;
  // 伏せた後で切り詰める。長いときは、頭（stderr の [ERROR]）と終わり（stdout の JSON のエラー）の両方を残す。
  const half = FAILURE_OUTPUT_LENGTH / 2;
  const text =
    output.length > FAILURE_OUTPUT_LENGTH ? `${output.slice(0, half)}\n…\n${output.slice(-half)}` : output;
  return `wrangler が失敗した（${detail}）\n${text}`;
}
