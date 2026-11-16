// 画面の JavaScript（public/assets/）が使う、htmx の型（tsconfig.json で読む）。public/ の外に置き、配らない。
// htmx.min.js は、ビルドのときに node_modules から写すだけで、型は使わない。使う関数だけを書く。
interface Htmx {
  ajax(verb: string, path: string, context: { target: string; swap: string }): Promise<unknown>;
}

interface Window {
  htmx?: Htmx;
}
