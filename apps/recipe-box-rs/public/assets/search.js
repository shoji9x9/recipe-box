// 一覧の検索の欄（data-search-input）で、日本語の入力（IME）の変換中は検索しない。
//
// 変換中も、打つたびに input が起きる。そのまま検索すると、確定する前の「おにg」のような文字で一覧が入れ替わり、
// 1 回ごとにブラウザの履歴も増える。そこで、変換中でない input と、変換を確定したとき（compositionend）にだけ、
// htmx が待つ search-input を起こす（templates/list.html の hx-trigger）。
// htmx のイベントの絞り込み（[...]）は、eval を禁じている（htmx-config の allowEval）ので使わない。
"use strict";

(() => {
  const INPUT = "input[data-search-input]";

  function notify(event) {
    const input = event.target;
    if (input instanceof HTMLInputElement && input.matches(INPUT)) {
      input.dispatchEvent(new Event("search-input"));
    }
  }

  // htmx がページを入れ替えても効くよう、document で受ける。
  document.addEventListener("input", (event) => {
    if (!event.isComposing) {
      notify(event);
    }
  });
  document.addEventListener("compositionend", notify);
})();
