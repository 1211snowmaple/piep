import type { LucideIcon } from "lucide-react";
import { BookOpen, CalendarDays, Clock3, Download, FileText, History, Images } from "lucide-react";
import type { PcWork } from "./pcApi";
import { longDate, readingMinutes } from "./workText";

export type WorkFact = { key: string; icon: LucideIcon; text: string };

/**
 * 作品の事実を文の続きのように一列に流す。数字が四つ五つしかないのに升目で区切ると、
 * 表を読まされる形になる。デスクトップ版の詳細の見出しと同じ並べ方。
 */
export function WorkFacts({ items }: { items: WorkFact[] }) {
  if (!items.length) return null;
  return <ul className="work-facts">{items.map(({ key, icon: Icon, text }) => <li key={key}><Icon aria-hidden="true" />{text}</li>)}</ul>;
}

/** 語ることがない項目は出さない。画像のない作品に「画像 0 枚」とは書かない。 */
export function workFacts(work: PcWork, pageCount?: number): WorkFact[] {
  const published = longDate(work.sourceCreatedAt);
  const savedAt = longDate(work.downloadedAt);
  const minutes = readingMinutes(work.textLength);
  const facts: WorkFact[] = [];
  if (published) facts.push({ key: "published", icon: CalendarDays, text: `公開 ${published}` });
  if (work.textLength > 0) facts.push({ key: "length", icon: FileText, text: `${work.textLength.toLocaleString("ja-JP")}字` });
  if (minutes) facts.push({ key: "minutes", icon: Clock3, text: minutes });
  if (work.assetCount > 0) facts.push({ key: "assets", icon: Images, text: `画像 ${work.assetCount.toLocaleString("ja-JP")}枚` });
  if (pageCount && pageCount > 1) facts.push({ key: "pages", icon: BookOpen, text: `${pageCount}ページ` });
  if (work.currentVersion > 1) facts.push({ key: "version", icon: History, text: `第${work.currentVersion}版` });
  if (savedAt) facts.push({ key: "saved", icon: Download, text: `保存 ${savedAt}` });
  return facts;
}
