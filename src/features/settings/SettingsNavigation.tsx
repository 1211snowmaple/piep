import { useState } from "react";
import { Card, NavLink, Text, TextInput } from "@mantine/core";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";

export interface SettingsItem { id: string; label: string; description: string; icon: LucideIcon; group: string; keywords?: string }

export function SettingsNavigation({ items, active, onSelect }: { items: SettingsItem[]; active: string; onSelect: (id: string) => void }) {
  const [query, setQuery] = useState("");
  const visible = items.filter((item) => `${item.label} ${item.description} ${item.keywords ?? ""}`.toLocaleLowerCase("ja-JP").includes(query.trim().toLocaleLowerCase("ja-JP")));
  return <Card p="sm" className="settings-nav" component="nav" aria-label="設定の項目">
    <TextInput size="sm" mb="sm" aria-label="設定項目を検索" placeholder="設定項目を検索" value={query} onChange={(event) => setQuery(event.currentTarget.value)} leftSection={<Icons.search size={IconSize.menu} />} />
    <div className="settings-nav__groups">
      {[...new Set(visible.map((item) => item.group))].map((group) => <div key={group} className="settings-nav__group">
        <Text size="xs" c="dimmed" fw={700} px="xs" py={6}>{group}</Text>
        {visible.filter((item) => item.group === group).map((item) => { const Icon = item.icon; return <NavLink component="button" type="button" key={item.id} active={active === item.id} aria-current={active === item.id ? "page" : undefined} label={item.label} description={item.description} leftSection={<Icon size={18} />} onClick={() => { onSelect(item.id); setQuery(""); }} />; })}
      </div>)}
    </div>
    {!visible.length && <Text size="sm" c="dimmed" p="xs">該当する設定はありません。</Text>}
  </Card>;
}
