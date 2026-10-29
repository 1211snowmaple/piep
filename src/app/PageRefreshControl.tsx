import { useCallback, useRef, useState } from "react";
import { ActionIcon, Tooltip } from "@mantine/core";
import { useHotkeys } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { errorMessage } from "@/lib/format";

/** Re-read visible data without discarding forms, scroll or native jobs. */
export function PageRefreshControl() {
  const client = useQueryClient();
  const running = useRef(false);
  const [refreshing, setRefreshing] = useState(false);
  const refresh = useCallback(async () => {
    if (running.current) return;
    running.current = true;
    setRefreshing(true);
    try {
      await client.invalidateQueries({ refetchType: "active" }, { cancelRefetch: false, throwOnError: true });
    } catch (error) {
      notifications.show({ color: "red", title: "画面を更新できません", message: errorMessage(error) });
    } finally {
      running.current = false;
      setRefreshing(false);
    }
  }, [client]);
  useHotkeys([["F5", () => void refresh()], ["mod+R", () => void refresh()]], [], true);
  return <Tooltip label="画面を更新（F5）"><ActionIcon variant="subtle" color="gray" aria-label="画面を更新" loading={refreshing} onClick={() => void refresh()}><Icons.retry size={IconSize.nav} /></ActionIcon></Tooltip>;
}
