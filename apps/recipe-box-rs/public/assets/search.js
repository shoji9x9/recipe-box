// 一覧の検索の欄（data-search-input）で、日本語の入力（IME）の変換中は検索しない。
//
// 変換中も、打つたびに input が起きる。そのまま検索すると、確定する前の「おにg」のような文字で一覧が入れ替わり、
// 1 回ごとにブラウザの履歴も増える。そこで、変換中でない input と、変換を確定したとき（compositionend）にだけ、
// htmx が待つ search-input を起こす（templates/list.html の hx-trigger）。
// ただし、変換中でも、入力欄の値の全体が ASCII だけなら、打ちながら検索する。Android のキーボード（Gboard など）は、
// 英字も単語ごとに変換中として打つので、そうしないと英字で打ちながら検索できなくなるためである。
// 日本語のローマ字入力の途中（「だn」など）は ASCII でない文字を含むので、確定まで待つ。値の一部が日本語のとき
// （確定した「カレー」の後ろに英字を変換中として打つときなど）も、確定まで待つ。
// 変換中の文字だけ（InputEvent.data）を見ないのは、単語ごとの変換では一部しか入らないことがあるためである。
// ブラウザによっては、確定したときに compositionend と、変換中でない input の両方が起きる（Safari など）。
// 値が同じなら htmx の changed が 1 回の検索にまとめる。
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

  // 印字できる ASCII の文字（空白を含む）だけの文字列か。
  const ASCII_ONLY = /^[\x20-\x7e]*$/;

  // htmx がページを入れ替えても効くよう、document で受ける。
  document.addEventListener("input", (event) => {
    if (!event.isComposing || ASCII_ONLY.test(event.target.value ?? "")) {
      notify(event);
    }
  });
  document.addEventListener("compositionend", notify);

  // 入力を消すボタン（data-search-clear）。欄を空にして絞り込まない一覧にし、欄にフォーカスを戻す。
  // ボタンは JavaScript が動かないときのために別のフォーム（templates/list.html の clear-search）を送るので、送らせない。
  // 検索は、打ったときと同じ search-input で起こす。changed が覚える前の値も空になり、同じ文字を打ち直しても検索する。
  document.addEventListener("click", (event) => {
    const button = event.target instanceof Element ? event.target.closest("button[data-search-clear]") : null;
    // button.form は form 属性の先（clear-search）を指すので、外側の検索のフォームから欄を探す。
    const input = button?.closest("form")?.querySelector(INPUT);
    if (!input) {
      return;
    }
    event.preventDefault();
    input.value = "";
    input.dispatchEvent(new Event("search-input"));
    input.focus();
  });
})();
