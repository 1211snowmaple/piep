// サイトの見た目をアプリに合わせる。
//
// 既定のテーマのまま出すと、青も書体も角丸もアプリと無関係な別物になる。
// piep を使っている人が同じ日に両方を見るので、そこが揃っていないと
// 「同じものの説明」に見えない。
//
// 値の出どころは一つだけ、`src/theme.ts` と `src/styles/app.css` である。
// ここで色を作らない。写すだけにする。
import DefaultTheme from "vitepress/theme";
import "./piep.css";

export default DefaultTheme;
