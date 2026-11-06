import { useEffect, useState } from "react";
import { cachedShelf, isLocalReadingLoaded, loadLocalReading, loadShelf, shelfChangedEvent, type RecentWork, type WorkIdentity } from "./localReading";

export { rememberReading, setReadingServer, type RecentWork } from "./localReading";

/** 読み終えたものは棚から外す。新しく読んだ順。記録は端末の SQLite から読む。 */
export function useReadingShelf(): RecentWork[] {
  const [items, setItems] = useState<RecentWork[]>(cachedShelf);
  useEffect(() => {
    let alive = true;
    let latest = 0;
    const update = () => {
      const request = ++latest;
      void loadShelf().then((next) => { if (alive && request === latest) setItems(next); });
    };
    update();
    window.addEventListener(shelfChangedEvent, update);
    return () => { alive = false; window.removeEventListener(shelfChangedEvent, update); };
  }, []);
  return items;
}

/**
 * 開く作品の端末の記録を読み込み、そろったかを返す。そろうまで画面を出さないので、
 * 「続きから読む」や開く位置が後から変わらない。履歴から戻ったときも同じ。
 */
export function useLocalReadingsReady(works: (WorkIdentity | null | undefined)[]): boolean {
  const [, setLoaded] = useState(0);
  const present = works.filter((work): work is WorkIdentity => Boolean(work));
  const key = present.map((work) => `${work.source}:${work.sourceId}`).join("|");
  useEffect(() => {
    let alive = true;
    for (const work of present) {
      if (!isLocalReadingLoaded(work)) void loadLocalReading(work).then(() => { if (alive) setLoaded((count) => count + 1); });
    }
    return () => { alive = false; };
  }, [key]);
  return present.every(isLocalReadingLoaded);
}
