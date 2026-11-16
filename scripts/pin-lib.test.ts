// scripts/pin-lib.ts の単体テスト（mise run test の vitest で実行する）。
import assert from "node:assert/strict";
import { describe, test } from "vitest";
import { pinOutcome } from "./pin-lib.ts";

// UPDATE と SELECT の 2 つの文の出力（wrangler d1 execute --json）を作る。
function output(rows: unknown[]): string {
  return JSON.stringify([{ results: [] }, { results: rows }]);
}

describe("pinOutcome", () => {
  test("望んだ値になっていれば done", () => {
    assert.equal(pinOutcome(true, output([{ pinned: 1 }])), "done");
    assert.equal(pinOutcome(false, output([{ pinned: 0 }])), "done");
  });

  test("値が違えば unchanged", () => {
    assert.equal(pinOutcome(true, output([{ pinned: 0 }])), "unchanged");
    assert.equal(pinOutcome(false, output([{ pinned: 1 }])), "unchanged");
    assert.equal(pinOutcome(true, output([{ pinned: null }])), "unchanged");
  });

  test("行が無ければ missing", () => {
    assert.equal(pinOutcome(true, output([])), "missing");
  });

  test("形が違えば undefined（ワークスペースが無いと取り違えない）", () => {
    for (const text of ["[]", "[{}]", output([{}]), output(["1"]), output([null]), "null"]) {
      assert.equal(pinOutcome(true, text), undefined, text);
    }
  });

  test("JSON として読めなければ、例外を投げる", () => {
    assert.throws(() => pinOutcome(true, "✘ [ERROR] not json"), SyntaxError);
  });
});
