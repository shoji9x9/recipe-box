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
//
// htmx は、search-input から 300 ms 待ってから、その時の欄の値で検索する。変換を始めた最初の英字（「k」など）で
// search-input が起きた後、待つ間に「かr」のような変換中の日本語になることがある。そこで、検索を送る直前
// （htmx:confirm）にも確かめ、変換中で ASCII でない文字を含むなら送らない。確定したときに改めて検索する。
//
// 入力を消すボタン（data-search-clear）も扱う。ブラウザが出す ×（Tab で選べない）の代わりに置いている。
//
// ES モジュールとして読み込む（base.html の <script type="module">）。判定の関数は、単体テスト
// （apps/recipe-box-rs/js-tests/）のために export する。型は JSDoc で書き、`tsc --noEmit` で確かめる。
// @ts-check

const INPUT = "input[data-search-input]";
const CLEAR = "button[data-search-clear]";

// 印字できる ASCII の文字（空白を含む）だけの文字列か。
const ASCII_ONLY = /^[\x20-\x7e]*$/;

/**
 * 検索の欄なら、その欄を返す。
 * @param {EventTarget | null} target
 * @returns {HTMLInputElement | null}
 */
function searchInput(target) {
  return target instanceof HTMLInputElement && target.matches(INPUT) ? target : null;
}

/**
 * htmx が待つ search-input を起こす。
 * @param {HTMLInputElement} input
 */
function search(input) {
  input.dispatchEvent(new Event("search-input"));
}

/**
 * input で検索するか。変換中でなければする。変換中は、欄の値の全体が ASCII だけのときだけする。
 * @param {Event} event
 * @param {string} value 欄の今の値
 */
export function searchesOnInput(event, value) {
  const composing = event instanceof InputEvent && event.isComposing;
  return !composing || ASCII_ONLY.test(value);
}

/**
 * 検索を送ってよいか。変換中で、欄の値が ASCII でない文字を含むなら送らない。
 * @param {boolean} composing 変換中か
 * @param {string} value 欄の今の値
 */
export function allowsRequest(composing, value) {
  return !composing || ASCII_ONLY.test(value);
}

/**
 * 入力を消すボタンが押されたら、外側の検索のフォームの欄を空にして絞り込まない一覧にし、欄にフォーカスを戻す。
 * ボタンは、JavaScript が動かないときのために別のフォーム（templates/list.html の clear-search）を送るので、送らせない。
 * 検索は、打ったときと同じ search-input で起こす。changed が覚える前の値も空になり、同じ文字を打ち直しても検索する。
 * @param {Event} event
 * @returns {boolean} 入力を消したか
 */
export function clearOnClick(event) {
  const button = event.target instanceof Element ? event.target.closest(CLEAR) : null;
  // button.form は form 属性の先（clear-search）を指すので、外側の検索のフォームから欄を探す。
  const input = button?.closest("form")?.querySelector(INPUT);
  if (!(input instanceof HTMLInputElement)) {
    return false;
  }
  event.preventDefault();
  input.value = "";
  search(input);
  input.focus();
  return true;
}

// 変換中の検索の欄。compositionstart から compositionend まで。
/** @type {WeakSet<HTMLInputElement>} */
const composing = new WeakSet();

// htmx がページを入れ替えても効くよう、document で受ける。
document.addEventListener("compositionstart", (event) => {
  const input = searchInput(event.target);
  if (input) {
    composing.add(input);
  }
});
document.addEventListener("input", (event) => {
  const input = searchInput(event.target);
  if (input && searchesOnInput(event, input.value)) {
    search(input);
  }
});
document.addEventListener("compositionend", (event) => {
  const input = searchInput(event.target);
  if (input) {
    composing.delete(input);
    search(input);
  }
});
document.addEventListener("htmx:confirm", (event) => {
  const input = searchInput(event.target);
  if (input && !allowsRequest(composing.has(input), input.value)) {
    event.preventDefault();
  }
});
document.addEventListener("click", clearOnClick);
