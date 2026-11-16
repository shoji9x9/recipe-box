// Workers だけにある組み込みの型（src/*.js の JSDoc の型を確かめる。tsconfig.json）。
// テストでは、代わりのもの（js-tests の FakeFixedLengthStream）を置く。

/** 流れる長さを決めたストリーム。長さが違えば、読む側が失敗する。 */
declare class FixedLengthStream extends TransformStream<Uint8Array, Uint8Array> {
  constructor(length: number | bigint);
}
