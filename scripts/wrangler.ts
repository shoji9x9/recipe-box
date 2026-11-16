// wrangler を動かす管理用のスクリプト（seed.ts・pin-workspace.ts）が共に使う部分。
// 失敗の文の伏せ字と、デモの ID の読み方を 1 か所にまとめる（写すと、片方だけ直して食い違うため）。
import { execFile } from "node:child_process";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify, stripVTControlCharacters } from "node:util";
import { type PathRule, pathPattern, redact as redactWith } from "./seed-lib.ts";

const run = promisify(execFile);

/** リポジトリのルート。 */
export const root = join(dirname(fileURLToPath(import.meta.url)), "..");
/** wrangler.toml のあるディレクトリ。wrangler はここで動かす。 */
export const appDir = join(root, "apps", "recipe-box-rs");
// 写真 1 枚ごとに pnpm を起こさないよう、pnpm install が入れた wrangler を直接動かす。
const wranglerBin = join(root, "node_modules", ".bin", "wrangler");
// ログで . と ~ に置き換える、リポジトリとホームのディレクトリのパス（端末の利用者名が入る。seed-lib.ts の pathPattern）。
const PATHS: PathRule[] = [
  [root, "."],
  [homedir(), "~"],
].flatMap(([path, replacement]): PathRule[] => {
  const pattern = pathPattern(path as string);
  return pattern === null ? [] : [[pattern, replacement as string]];
});

/** ワークスペースの ID の形（16 進数の小文字 32 文字）。 */
export const ID_PATTERN = /^[0-9a-f]{32}$/;

/** wrangler の --local（ローカルの D1・R2）か --remote（本番）。 */
export type Target = "--local" | "--remote";

// 表示やエラーの文から、ID の形の値を伏せ、リポジトリとホームのパスを . と ~ に置き換える（seed-lib.ts の redact）。
export function redact(text: unknown): string {
  return redactWith(text, PATHS);
}

// デモの ID を標準入力の 1 行目から読み、空白を除く。形は呼ぶ側が ID_PATTERN で確かめる。
// デモの ID は公開しないので、引数ではなく標準入力で受け取る（引数は mise の表示やシェルの履歴に残る）。
export function readIdFromStdin(): string {
  return (readFileSync(0, "utf8").split("\n")[0] ?? "").replace(/\s/g, "");
}

// wrangler を動かす。出力には ID が入るので、失敗したときだけ、ID を伏せて表示する。
export async function wrangler(target: Target, ...wranglerArgs: string[]): Promise<string> {
  try {
    const { stdout } = await run(wranglerBin, [...wranglerArgs, target], {
      cwd: appDir,
      // 色の制御文字を出させない（--json の出力を JSON として読むためと、制御文字で値が分かれると伏せられなくなるため）。
      env: { ...process.env, FORCE_COLOR: "0", NO_COLOR: "1" },
      maxBuffer: 16 * 1024 * 1024,
    });
    return stdout;
  } catch (error) {
    // execFile の失敗は、出力（stdout・stderr）と、終了コードかシグナルを持つ。
    const failure = error as {
      stdout?: string;
      stderr?: string;
      code?: string | number | null;
      signal?: string | null;
      name?: string;
    };
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
    );
    // 終了コードもシグナルも無ければ（execFile ではふつう起きない）、エラーの名前を添える。
    const detail = [failure.code, failure.signal].filter((value) => value != null).join("・") || failure.name;
    // 伏せた後で切り詰める。長いときは、頭（stderr の [ERROR]）と終わり（stdout の JSON のエラー）の両方を残す。
    const text = output.length > 4000 ? `${output.slice(0, 2000)}\n…\n${output.slice(-2000)}` : output;
    throw new Error(`wrangler が失敗した（${detail}）\n${text}`);
  }
}
