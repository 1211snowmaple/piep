import type { ReactNode } from "react";
import { Box, type MantineSpacing, type StyleProp } from "@mantine/core";

/**
 * カード・フォーム・ダイアログの下端に置く操作の行。
 *
 * 決定は右端に揃える。どの画面でも、押して確定するものを探す先が同じになる。
 * 主操作は `children` の最後に置く。`start` には決定ではないもの（選択の道具や
 * 補足の一文）を置く。幅が足りずに折り返しても、右の塊は右に残る。
 */
export function ActionRow({ start, children, mt }: { start?: ReactNode; children: ReactNode; mt?: StyleProp<MantineSpacing> }) {
  return (
    <Box className="action-row" mt={mt}>
      {start && <div className="action-row__start">{start}</div>}
      <div className="action-row__end">{children}</div>
    </Box>
  );
}
