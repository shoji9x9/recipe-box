// scripts/wrangler-lib.ts の単体テスト（mise run test の vitest で実行する）。
// wrangler の失敗の文は公開の Actions のログに出るので、ID とパスが残らないことを確かめる。
import assert from "node:assert/strict";
import { describe, test } from "vitest";
import { type PathRule, pathPattern } from "./seed-lib.ts";
import { FAILURE_OUTPUT_LENGTH, wranglerFailureMessage } from "./wrangler-lib.ts";

const ID = "0123456789abcdef0123456789abcdef";
const HOME = "/home/someone";
const PATHS: PathRule[] = [[pathPattern(HOME) ?? assert.fail("パスの形"), "~"]];

describe("wranglerFailureMessage", () => {
  test("stderr と stdout の ID とパスを伏せ、終了コードを添える", () => {
    const message = wranglerFailureMessage(
      { stderr: `[ERROR] ${ID} が無い`, stdout: `{"text": "${HOME}/x"}`, code: 1 },
      PATHS,
    );
    assert.equal(message, `wrangler が失敗した（1）\n[ERROR] *** が無い\n{"text": "~/x"}`);
  });

  test("制御文字に挟まれた ID も伏せる", () => {
    const split = `${ID.slice(0, 16)}\u001b[0m${ID.slice(16)}`;
    const message = wranglerFailureMessage({ stderr: `\u001b[31m${split}\u001b[0m`, code: 1 }, PATHS);
    assert.equal(message, "wrangler が失敗した（1）\n***");
  });

  test("ログのファイルの場所の行と、空の出力を除く", () => {
    const message = wranglerFailureMessage(
      { stderr: `理由\nLogs were written to "${HOME}/.wrangler/logs/a.log"`, stdout: "  \n", code: 1 },
      PATHS,
    );
    assert.equal(message, "wrangler が失敗した（1）\n理由");
  });

  test("シグナルを添え、終了コードもシグナルも無ければエラーの名前を添える", () => {
    assert.equal(wranglerFailureMessage({ code: null, signal: "SIGTERM" }, PATHS), "wrangler が失敗した（SIGTERM）\n");
    assert.equal(wranglerFailureMessage({ code: "ENOENT" }, PATHS), "wrangler が失敗した（ENOENT）\n");
    assert.equal(wranglerFailureMessage({ name: "Error" }, PATHS), "wrangler が失敗した（Error）\n");
  });

  test("長い出力は、伏せた後で頭と終わりを残して切り詰める", () => {
    // 伏せる前に切ると、境目で ID が切れて伏せられずに残る。伏せた後で切れば、境目に ID の一部は出ない。
    const half = FAILURE_OUTPUT_LENGTH / 2;
    // ID が、伏せる前の文の切り詰めの境目（頭から half 文字目）をまたぐようにする。埋める文字は、伏せる対象（16 進数と -）を避ける。
    const head = `[ERROR] 頭${"x".repeat(half - 9 - 16)}`;
    const stderr = `${head}${ID}${"z".repeat(FAILURE_OUTPUT_LENGTH)}終わり`;
    const message = wranglerFailureMessage({ stderr, code: 1 }, PATHS);
    const [first, ...rest] = message.split("\n");
    assert.equal(first, "wrangler が失敗した（1）");
    const text = rest.join("\n");
    assert.ok(!text.includes(ID.slice(0, 16)));
    assert.ok(text.includes("***"));
    assert.ok(text.startsWith("[ERROR] 頭"));
    assert.ok(text.endsWith("終わり"));
    assert.equal(text.length, FAILURE_OUTPUT_LENGTH + 3);
  });

  test("切り詰めの長さちょうどなら切らない", () => {
    const stderr = "x".repeat(FAILURE_OUTPUT_LENGTH);
    assert.equal(wranglerFailureMessage({ stderr, code: 1 }, PATHS), `wrangler が失敗した（1）\n${stderr}`);
  });
});
