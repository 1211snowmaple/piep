import type { ReactNode } from "react";
import { Box, Group, Text, Title } from "@mantine/core";
import { BetaBadge } from "@/components/BetaBadge";

/** 設定の各項目の見出し。β版の項目も、印が付く以外は同じ形にする。 */
export function SectionIntro({ title, description, beta = false }: { title: string; description: ReactNode; beta?: boolean }) {
  return <Box mb="lg">
    <Group gap="xs"><Title order={2}>{title}</Title>{beta && <BetaBadge size="sm" />}</Group>
    <Text c="dimmed" size="sm" mt={5}>{description}</Text>
  </Box>;
}
