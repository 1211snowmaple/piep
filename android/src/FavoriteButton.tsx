import { Icons } from "./icons";
import { useFavorite } from "./favorites";
import type { PcWork } from "./pcApi";

/** 作品のお気に入りを、端末の付け外しを含めた今の値で出す。押せるかどうかもここで決まる。 */
export function WorkFavoriteButton({ work, className, readOnly = false }: { work: PcWork; className: string; readOnly?: boolean }) {
  const { favorite, busy, onToggle } = useFavorite(work);
  return <FavoriteButton favorite={favorite} className={className} busy={busy} onToggle={readOnly ? undefined : onToggle} />;
}

/**
 * お気に入りの印。カード・詳細・読書画面で同じものを使う。
 *
 * 切り替えの口（`onToggle`）が渡されたときだけ押せるボタンにする。渡されなければ、
 * お気に入りのときに赤い印を出すだけで、押せそうな灰色の印は出さない。
 * サーバーへ書けないのに押せる形を見せない。
 */
export function FavoriteButton({ favorite, className, onToggle, busy = false }: {
  favorite: boolean;
  className: string;
  onToggle?: (next: boolean) => void;
  busy?: boolean;
}) {
  if (onToggle) {
    return <button type="button" className={className} data-tone="favorite" data-on={favorite || undefined} aria-pressed={favorite} disabled={busy} aria-label={favorite ? "お気に入りを外す" : "お気に入りに追加"} title={favorite ? "お気に入りを外す" : "お気に入りに追加"} onClick={() => onToggle(!favorite)}><Icons.favorite aria-hidden="true" /></button>;
  }
  if (!favorite) return null;
  return <span className={className} data-tone="favorite" data-on="true" role="img" aria-label="お気に入り" title="お気に入り"><Icons.favorite aria-hidden="true" /></span>;
}
