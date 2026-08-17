import {
  Activity,
  AlertTriangle,
  AppWindow,
  Archive,
  ArchiveRestore,
  ArrowDown,
  ArrowDownUp,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  Ban,
  EyeOff,
  Bookmark,
  BookMarked,
  Braces,
  BookmarkPlus,
  BookCheck,
  BookOpen,
  BookPlus,
  BookText,
  CalendarDays,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  CircleHelp,
  ClipboardPaste,
  Clock3,
  Copy as CopyIcon,
  Database,
  Download,
  Edit3,
  Ellipsis,
  Expand,
  Eye,
  ExternalLink,
  File,
  FileCode2,
  FileImage,
  FileText,
  Filter,
  FolderOpen,
  Gauge,
  GalleryHorizontalEnd,
  Globe2,
  Grid2X2,
  GripVertical,
  HardDrive,
  Heading2,
  Heart,
  History,
  Home,
  ImageDown,
  ImagePlus,
  Infinity,
  Images,
  Inbox,
  Info,
  KeyRound,
  LayoutTemplate,
  Library,
  LibraryBig,
  Link2,
  List,
  ListChecks,
  ListRestart,
  LockKeyhole,
  Maximize2,
  Menu,
  Minimize2,
  Minus,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Palette,
  Pause,
  Pilcrow,
  Play,
  Plus,
  Quote,
  RefreshCw,
  RotateCcw,
  RotateCw,
  Save,
  ScanLine,
  Search,
  SearchCheck,
  Settings,
  Settings2,
  ShieldCheck,
  Shrink,
  Sparkles,
  Sun,
  Tag,
  Trash2,
  TriangleAlert,
  Type,
  Upload,
  UserRound,
  Users,
  X,
  XCircle,
  type LucideIcon,
} from "lucide-react";

/**
 * Every icon the app uses, named for what it means rather than what it draws.
 *
 * The point is that one role has one icon. Adding a work to the EPUB queue was
 * drawn four different ways - a book, a book with text, an open book and a
 * download arrow - depending on which screen you were looking at, because each
 * screen picked its own from the icon set. Importing a role from here instead
 * of a shape from `lucide-react` makes that impossible: to change how something
 * looks you change it once, and everywhere that means the same thing follows.
 *
 * Adding an icon means adding a role here first. A `lucide-react` import
 * anywhere else in `src` is a bug.
 */
export const Icons = {
  // ---- Works ------------------------------------------------------------
  /** Open a work in the reader. */
  read: BookOpen,
  /** Mark, or already marked, as a favourite. */
  favorite: Heart,
  /** Watch a work for updates at its source. */
  watch: RefreshCw,
  /** Open a work's detail page. */
  workDetail: Library,
  /** Edit a work's text. */
  edit: Edit3,
  /** Character count of a work. */
  textLength: FileText,
  /** Images attached to a work. */
  assets: Images,
  /** The date a work was published at its source. */
  publishedDate: CalendarDays,
  /** A work matched the current search. */
  searchMatch: SearchCheck,

  // ---- EPUB -------------------------------------------------------------
  /** The EPUB queue as a destination. */
  epub: BookText,
  /** Add a work to the EPUB queue. */
  epubAdd: BookPlus,
  /** Already in the EPUB queue. */
  epubQueued: BookCheck,
  /** EPUB templates. */
  epubTemplate: FileCode2,
  /** Image handling inside an EPUB export. */
  epubImages: ImageDown,
  /** Seeing a template rendered before exporting with it. */
  epubPreview: Eye,
  /** Duplicating a template into a new one. */
  epubDuplicate: CopyIcon,
  /** What a template can place: the fields a work provides. */
  epubDataField: Braces,
  /** The structure of a book: which pages it has and in what order. */
  epubStructure: LayoutTemplate,

  // ---- People, series and tags -------------------------------------------
  /** One author or creator. */
  person: UserRound,
  /** The authors listing. */
  people: Users,
  /** One series, and the series listing. */
  series: Library,
  /** The whole library. */
  library: LibraryBig,
  /** A tag. */
  tag: Tag,
  /** One saved search in a list of them. */
  savedSearchItem: BookMarked,

  // ---- Navigation --------------------------------------------------------
  home: Home,
  search: Search,
  /** Saved search conditions. */
  savedSearch: Bookmark,
  /** Save the current conditions as a named search. */
  saveSearch: BookmarkPlus,
  /** Acquire works from a service. */
  collect: Download,
  /** Check sources for new or changed works. */
  updates: RefreshCw,
  /** Running and finished background work. */
  activity: Activity,
  /** The operation history listing. */
  history: ListRestart,
  settings: Settings,
  /** Measurement and maintenance of the library. */
  diagnostics: Gauge,
  help: CircleHelp,
  appMenu: Menu,

  // ---- Directions and disclosure -----------------------------------------
  back: ArrowLeft,
  forward: ArrowRight,
  up: ArrowUp,
  down: ArrowDown,
  previous: ChevronLeft,
  next: ChevronRight,
  expand: ChevronDown,
  sidebarClose: PanelLeftClose,
  sidebarOpen: PanelLeftOpen,
  panelClose: PanelRightClose,
  panelOpen: PanelRightOpen,
  /** Give a panel the whole window. */
  maximize: Maximize2,
  /** Return a panel to its place in the app. */
  minimize: Minimize2,
  fullscreenEnter: Expand,
  fullscreenExit: Shrink,
  drag: GripVertical,

  // ---- Actions -----------------------------------------------------------
  add: Plus,
  remove: Minus,
  confirm: Check,
  cancel: X,
  delete: Trash2,
  save: Save,
  retry: RotateCw,
  undo: RotateCcw,
  pause: Pause,
  resume: Play,
  stop: Ban,
  /** 「今後この候補を出さない」。削除ではなく、見えなくする操作。 */
  hide: EyeOff,
  more: Ellipsis,
  sort: ArrowDownUp,
  filter: Filter,
  viewGrid: Grid2X2,
  viewList: List,
  /** Loading a listing on and on as it is scrolled. */
  pagingContinuous: Infinity,
  /** Stepping through a listing one numbered page at a time. */
  pagingNumbered: GalleryHorizontalEnd,
  select: ListChecks,
  paste: ClipboardPaste,
  externalLink: ExternalLink,
  link: Link2,
  openFolder: FolderOpen,

  // ---- Status ------------------------------------------------------------
  success: CheckCircle2,
  failure: XCircle,
  warning: TriangleAlert,
  /** A problem that stopped a screen from rendering. */
  error: AlertTriangle,
  /** A caution inside otherwise working content. */
  notice: CircleAlert,
  info: Info,
  pending: Clock3,
  empty: Inbox,

  // ---- Storage and data ---------------------------------------------------
  database: Database,
  storage: HardDrive,
  archive: Archive,
  restore: ArchiveRestore,
  import: Upload,
  export: Download,
  file: File,
  imageFile: FileImage,
  versionHistory: History,
  optimize: Sparkles,

  // ---- Appearance and settings -------------------------------------------
  themeLight: Sun,
  themeDark: Moon,
  appearance: Palette,
  credentials: KeyRound,
  secure: ShieldCheck,
  desktopApp: AppWindow,
  /** 内蔵ブラウザで開く。保存（下向き矢印）とは別の意味なので絵も分ける。 */
  inAppBrowser: AppWindow,
  browser: Globe2,
  secureConnection: LockKeyhole,
  readerSettings: Settings2,
  typography: Type,

  // ---- Editor blocks -------------------------------------------------------
  paragraph: Pilcrow,
  heading: Heading2,
  quote: Quote,
  separator: ScanLine,
  insertImage: ImagePlus,
} as const satisfies Record<string, LucideIcon>;

export type IconRole = keyof typeof Icons;

/**
 * The sizes icons are drawn at.
 *
 * Picking a number per call site is how a row ends up with a 12px icon beside a
 * 15px one. These are the only sizes the app uses.
 */
export const IconSize = {
  /** Beside small print: metadata rows, badges. */
  inline: 13,
  /** Inside menu items and compact buttons. */
  menu: 15,
  /** Standalone controls and card actions. */
  action: 17,
  /** Sidebar rows and header controls. */
  nav: 18,
  /** Section headings and empty states. */
  feature: 22,
  /** The glyph inside a large placeholder tile. */
  hero: 30,
  /** The stand-in inside an avatar, where no picture was saved. */
  avatar: 42,
} as const;

export type { LucideIcon };
