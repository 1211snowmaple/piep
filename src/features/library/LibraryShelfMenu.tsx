import { ActionIcon, Menu, Tooltip } from "@mantine/core";
import { useLibrarySidebarVisibility } from "@/app/librarySidebarPreferences";
import { LIBRARY_SHELVES } from "@/app/libraryShelves";
import { useAppNavigate } from "@/app/router";
import { Icons, IconSize } from "@/lib/icons";

/** Hidden sidebar shortcuts remain reachable without changing the setting. */
export function LibraryShelfMenu() {
  const [visible] = useLibrarySidebarVisibility();
  const navigate = useAppNavigate();
  if (Object.values(visible).every(Boolean)) return null;
  return <Menu position="bottom-start">
    <Menu.Target><Tooltip label="ライブラリの表示対象"><ActionIcon variant="subtle" color="gray" aria-label="ライブラリの表示対象"><Icons.library size={IconSize.nav} /></ActionIcon></Tooltip></Menu.Target>
    <Menu.Dropdown>
      {LIBRARY_SHELVES.map((shelf) => <Menu.Item key={shelf.value} onClick={() => navigate(`/library${shelf.search ? `?${shelf.search}` : ""}`)}>{shelf.label}</Menu.Item>)}
      <Menu.Item onClick={() => navigate("/library?acquisition=attention")}>要確認</Menu.Item>
    </Menu.Dropdown>
  </Menu>;
}
