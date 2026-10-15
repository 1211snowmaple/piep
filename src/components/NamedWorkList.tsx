import { Stack, Text } from "@mantine/core";
import { formatNumber } from "@/lib/format";

/** 確認の窓に題名を並べる上限。これを超えたぶんは数で言う。 */
const CONFIRM_LIST_LIMIT = 12;

/**
 * 何に対する操作なのかを、題名で見せる。
 *
 * 取り返しのつかない操作の確認に「{n}件を削除します」とだけ書いていた画面が
 * いくつもあった。**数は対象を言い当てていない。** 選んだ覚えのないものが
 * 混ざっていても、押す前に気づく手がかりが一つも無い。
 *
 * 全部並べはしない。100件の削除で100行を読ませても、結局は読まれずに押される。
 * 先頭を出して、残りは数で言う — 少なくとも「思っていたものと違う」ことには
 * 気づける。
 */
export function NamedWorkList({ works }: { works: { title: string; authorName?: string | null }[] }) {
  if (works.length === 0) return null;
  const shown = works.slice(0, CONFIRM_LIST_LIMIT);
  const hidden = works.length - shown.length;
  return (
    <Stack gap={2} className="confirm-work-list">
      {shown.map((work, index) => (
        // 題名は折り返して最後まで出す。1行で切ると、同じ書き出しで始まる
        // 作品が**まったく同じ文字列**になり、何を消すのか見分けられない。
        // 窓が縦に伸びすぎないよう、この一覧そのものに高さの上限がある。
        <Text key={`${work.title}-${index}`} size="xs">
          ・{work.title}
          {work.authorName ? <Text span c="dimmed">（{work.authorName}）</Text> : null}
        </Text>
      ))}
      {hidden > 0 && <Text size="xs" c="dimmed">ほか{formatNumber(hidden)}件</Text>}
    </Stack>
  );
}
