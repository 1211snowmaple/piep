import type { ReactNode } from "react";
import { Box, Group, Text } from "@mantine/core";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";

interface NoteProps {
  children: ReactNode;
  /** Names what the note is about, when the first line is not enough. */
  title?: string;
  icon?: LucideIcon;
  mt?: string | number;
}

/**
 * Prose that explains how something works.
 *
 * These used to be `Alert`s, and every one of them picked its own colour: the
 * same kind of sentence appeared on blue, on green and on grey across the
 * settings screens, which made the colour look like it meant something when it
 * did not. An explanation is not a state - nothing has succeeded, failed or
 * needs attention - so it carries no colour at all, and `Alert` is left to the
 * cases that genuinely have one.
 */
export function Note({ children, title, icon: Icon = Icons.help, mt }: NoteProps) {
  return (
    <Box className="note" mt={mt}>
      <Group gap={10} wrap="nowrap" align="flex-start">
        <Icon size={IconSize.menu} className="note__icon" aria-hidden />
        <Box miw={0}>
          {title && <Text size="sm" fw={700} className="note__title">{title}</Text>}
          <Text size="sm" c="dimmed" className="note__body">{children}</Text>
        </Box>
      </Group>
    </Box>
  );
}
