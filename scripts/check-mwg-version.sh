#!/usr/bin/env bash
# modern-web-guidance のスキル（SKILL.md の --skill-version）と、package.json で入れたパッケージの版がそろっているかを確かめる。
# Dependabot がパッケージだけを上げたときに気づくため（AGENTS.md の「スキル」）。先に pnpm install が要る。
set -euo pipefail
skill=$(grep -oE -- '--skill-version [0-9_a-f-]+' .claude/skills/modern-web-guidance/SKILL.md | head -n 1 | cut -d' ' -f2)
package=$(tr -d '[:space:]' < node_modules/modern-web-guidance/skills/modern-web-guidance/skill-version.txt)
if [[ -z "$skill" || "$skill" != "$package" ]]; then
  echo "modern-web-guidance のスキル（${skill:-不明}）とパッケージ（$package）の版が違います。同じ版のスキルを入れ直してください（AGENTS.md の「スキル」）" >&2
  exit 1
fi
