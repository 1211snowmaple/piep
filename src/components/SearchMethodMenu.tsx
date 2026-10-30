import { ActionIcon, Box, Menu, Text } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { searchModes } from "@/lib/searchPreferences";
import type { LibrarySearchMode } from "@/types/library";

/** The method belongs to the search field, including inside a dialog. */
export function SearchMethodMenu({ value, onChange, includeHistory, onHistoryChange }: {
  value: LibrarySearchMode;
  onChange: (value: LibrarySearchMode) => void;
  includeHistory: boolean;
  onHistoryChange?: (value: boolean) => void;
}) {
  const selected = searchModes.find((mode) => mode.value === value)!;
  return <Menu position="bottom-start" width={270} zIndex={1200} withinPortal>
    <Menu.Target><ActionIcon variant="subtle" color={value === "smart" ? "gray" : "piep"} className="search-method-control" title={`検索方法：${selected.label}`} aria-label={`検索方法：${selected.label}`} onClick={(event) => event.stopPropagation()}><Icons.search size={IconSize.action} /><Icons.expand size={IconSize.inline} /></ActionIcon></Menu.Target>
    <Menu.Dropdown onKeyDown={(event) => event.stopPropagation()}>
      <Menu.Label>検索方法</Menu.Label>
      {searchModes.map((mode) => <Menu.Item key={mode.value} disabled={mode.value === "semantic" && includeHistory} onClick={() => onChange(mode.value)} leftSection={<Icons.confirm size={IconSize.menu} opacity={value === mode.value ? 1 : 0} />}><Box><Text size="sm" fw={value === mode.value ? 650 : 400}>{mode.label}</Text><Text size="xs" c="dimmed">{mode.description}</Text></Box></Menu.Item>)}
      {onHistoryChange && <><Menu.Divider /><Menu.Item closeMenuOnClick={false} leftSection={<Icons.versionHistory size={IconSize.menu} />} rightSection={includeHistory ? <Icons.confirm size={IconSize.menu} /> : undefined} onClick={() => onHistoryChange(!includeHistory)}>過去版も含める</Menu.Item></>}
    </Menu.Dropdown>
  </Menu>;
}
