// scripts/mwg.sh と scripts/check-mwg-version.sh の単体テスト（mise run test で実行する）。
// mwg.sh のサブコマンドの判定は、install・update を断る唯一の門なので、形ごとに確かめる。
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { after, before, describe, test } from "node:test";

const scripts = resolve(import.meta.dirname);

describe("mwg.sh", () => {
  let dir;
  let env;

  // 本物のツールを動かさないよう、引数と DISABLE_TELEMETRY を出すだけの pnpm を PATH の先頭に置く。
  before(() => {
    dir = mkdtempSync(join(tmpdir(), "mwg-test-"));
    const pnpm = join(dir, "pnpm");
    writeFileSync(pnpm, '#!/usr/bin/env bash\necho "telemetry=${DISABLE_TELEMETRY:-} args=$*"\n');
    chmodSync(pnpm, 0o755);
    env = { ...process.env, PATH: `${dir}:${process.env.PATH}` };
    delete env.DISABLE_TELEMETRY;
  });
  after(() => rmSync(dir, { recursive: true, force: true }));

  const run = (...args) => spawnSync(join(scripts, "mwg.sh"), args, { env, encoding: "utf8" });

  for (const args of [
    ["search", "dialog"],
    ["list"],
    ["retrieve", "dialog"],
    ["--skill-version", "2026_10_30-58d055df", "search", "dialog"],
    ["--skill-version=2026_10_30-58d055df", "retrieve", "dialog"],
    ["-v"],
    ["-h"],
  ]) {
    test(`${args.join(" ")} は、利用統計を止めて固定した版で動かす`, () => {
      const result = run(...args);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout.trim(), `telemetry=1 args=exec modern-web-guidance ${args.join(" ")}`);
    });
  }

  for (const args of [
    ["install"],
    ["update"],
    ["uninstall"],
    ["legacy-scan", "."],
    ["set-allow-origin-trials"],
    // --skill-version の値を読み飛ばした後のサブコマンドで決める。
    ["--skill-version", "search", "install"],
    ["--skill-version=x", "update"],
    // -- は、オプションとして読み飛ばす（ツールも同じく、最初の位置の引数をサブコマンドにする）。
    ["--", "install"],
    ["-v", "install"],
  ]) {
    test(`${args.join(" ")} は断る`, () => {
      const result = run(...args);
      assert.equal(result.status, 2);
      assert.equal(result.stdout, "");
      assert.match(result.stderr, /search・list・retrieve だけ/);
    });
  }
});

describe("check-mwg-version.sh", () => {
  let dir;

  before(() => {
    dir = mkdtempSync(join(tmpdir(), "mwg-version-test-"));
  });
  after(() => rmSync(dir, { recursive: true, force: true }));

  // dir の下に、スキルの SKILL.md とパッケージの skill-version.txt を置いて実行する（どちらも null なら置かない）。
  const check = (skill, packageVersion) => {
    const root = mkdtempSync(join(dir, "case-"));
    if (skill !== null) {
      mkdirSync(join(root, ".claude/skills/modern-web-guidance"), { recursive: true });
      writeFileSync(join(root, ".claude/skills/modern-web-guidance/SKILL.md"), skill);
    }
    if (packageVersion !== null) {
      const packageDir = join(root, "node_modules/modern-web-guidance/skills/modern-web-guidance");
      mkdirSync(packageDir, { recursive: true });
      writeFileSync(join(packageDir, "skill-version.txt"), packageVersion);
    }
    return spawnSync(join(scripts, "check-mwg-version.sh"), [], { cwd: root, encoding: "utf8" });
  };
  const skillWith = (version) => `npx -y modern-web-guidance@latest search "<query>" --skill-version ${version}\n`;

  test("版がそろっていれば通る（CRLF の SKILL.md と、改行付きの版のファイルも）", () => {
    for (const [skill, packageVersion] of [
      [skillWith("2026_10_30-58d055df"), "2026_10_30-58d055df"],
      [skillWith("2026_10_30-58d055df").replace("\n", "\r\n"), "2026_10_30-58d055df\n"],
    ]) {
      const result = check(skill, packageVersion);
      assert.equal(result.status, 0, result.stderr);
    }
  });

  test("版が違えば、両方の版を出して失敗する", () => {
    const result = check(skillWith("2026_10_30-58d055df"), "2026_11_06-909bc374");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /2026_10_30-58d055df.*2026_11_06-909bc374/);
  });

  test("SKILL.md から版を読めなければ失敗する", () => {
    const result = check("no version here\n", "2026_10_30-58d055df");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /--skill-version を読めません/);
  });

  test("SKILL.md が無ければ失敗する", () => {
    const result = check(null, "2026_10_30-58d055df");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /SKILL\.md がありません/);
  });

  test("パッケージの版のファイルが無ければ失敗する", () => {
    const result = check(skillWith("2026_10_30-58d055df"), null);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /skill-version\.txt がありません/);
  });
});
