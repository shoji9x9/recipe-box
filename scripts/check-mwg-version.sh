#!/usr/bin/env bash
# modern-web-guidance のスキル（SKILL.md の --skill-version）と、package.json で入れたパッケージの版がそろっているかを確かめる。
# Dependabot がパッケージだけを上げたときに気づくため（AGENTS.md の「スキル」）。先に pnpm install が要る。
set -euo pipefail
skill_file=.claude/skills/modern-web-guidance/SKILL.md
package_file=node_modules/modern-web-guidance/skills/modern-web-guidance/skill-version.txt
if [[ ! -f "$package_file" ]]; then
  echo "$package_file がありません。先に pnpm install を実行してください" >&2
  exit 1
fi
skill=$(grep -oE -- '--skill-version[ =][^ `"]+' "$skill_file" | head -n 1 | sed -E 's/^--skill-version[ =]//' || true)
package=$(tr -d '[:space:]' < "$package_file")
if [[ -z "$skill" ]]; then
  echo "$skill_file から --skill-version を読めません。書き方が変わっていないか確かめてください" >&2
  exit 1
fi
if [[ "$skill" != "$package" ]]; then
  echo "modern-web-guidance のスキル（$skill）とパッケージ（$package）の版が違います。同じ版のスキルを入れ直してください（AGENTS.md の「スキル」）" >&2
  exit 1
fi
