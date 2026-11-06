import {
  BookOpen,
  BookPlus,
  CalendarDays,
  Ellipsis,
  FileText,
  GalleryHorizontalEnd,
  Heart,
  History,
  Images,
  Info,
  Library,
  LibraryBig,
  UserRound,
  Users,
  type LucideIcon,
} from "lucide-react";

/**
 * 意味ごとのアイコン。デスクトップ版の `src/lib/icons.ts` と同じ役割に同じ図像を当てる。
 * 同じものを画面ごとに別の絵で描くと、別のものに見えるため。新しく使うときはここに役割を足す。
 */
export const Icons = {
  read: BookOpen,
  favorite: Heart,
  textLength: FileText,
  assets: Images,
  publishedDate: CalendarDays,
  versionHistory: History,
  /** 本棚の「作品」のタブ。デスクトップ版の作品タブと同じ絵。 */
  works: BookPlus,
  person: UserRound,
  people: Users,
  series: Library,
  library: LibraryBig,
  collection: GalleryHorizontalEnd,
  /** 作品情報を開く。一覧のカード、読書画面のどちらからでも同じ絵にする。 */
  workInfo: Info,
  more: Ellipsis,
} as const satisfies Record<string, LucideIcon>;
