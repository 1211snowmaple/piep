//! 読書と編集の問い合わせ。
//!
//! `mod.rs` から切り出した。あの一枚は 13,700 行あり、`impl Database` だけで
//! 163 個の関数を抱えている。**割り方はすでに前例があり**（`collections.rs`、
//! `collection_sweep.rs`、`archive_state.rs`、`assist_inputs.rs`）、本体が
//! 割られていないだけだった。`impl` は複数のファイルに分けて書けるので、
//! 公開 API は変えずに置き場所だけを移している。
//!
//! 一度に全部は割らない。何が壊れたのかを言えなくなる。

#[cfg(test)]
pub(super) use piep_reader_render::pages::folded_match_positions;
