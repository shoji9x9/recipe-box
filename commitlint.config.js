// commitlint の設定。commit message と PR のタイトルを Conventional Commits の形にそろえる。
// lefthook の commit-msg フックと、CI の PR のタイトルの検査から使う。
/** @type {import('@commitlint/types').UserConfig} */
export default {
  extends: ["@commitlint/config-conventional"],
  rules: {
    // 件名は日本語で書くので、大文字と小文字の規則は使わない。
    "subject-case": [0],
  },
};
