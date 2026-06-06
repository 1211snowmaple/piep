// SVGアイコンコンポーネント一元管理

export function HomeIcon() {
  return (
    <svg className="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M3 9l9-7 9 7v11a2 2 0 01-2 2H5a2 2 0 01-2-2z" />
      <polyline points="9,22 9,12 15,12 15,22" />
    </svg>
  );
}

export function PaletteIcon() {
  return (
    <svg className="nav-icon brand-icon pixiv" viewBox="0 0 24 24" fill="none" style={{ width: '18px', height: '18px', flexShrink: 0 }}>
      <circle cx="12" cy="12" r="10" fill="#0096FA" />
      <text x="50%" y="50%" fill="#FFFFFF" fontSize="12" fontWeight="800" textAnchor="middle" fontFamily="'Inter', system-ui, sans-serif" dominantBaseline="central">P</text>
    </svg>
  );
}

export function HeartIcon() {
  return (
    <svg className="nav-icon brand-icon fanbox" viewBox="0 0 24 24" fill="none" style={{ width: '18px', height: '18px', flexShrink: 0 }}>
      <circle cx="12" cy="12" r="10" fill="#F2C624" />
      <text x="50%" y="50%" fill="#FFFFFF" fontSize="12" fontWeight="800" textAnchor="middle" fontFamily="'Inter', system-ui, sans-serif" dominantBaseline="central">F</text>
    </svg>
  );
}

export function PixivIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" style={{ width: "18px", height: "18px", flexShrink: 0 }}>
      <circle cx="12" cy="12" r="10" fill="#0096FA" />
      <text x="50%" y="50%" fill="#FFFFFF" fontSize="12" fontWeight="800" textAnchor="middle" fontFamily="'Inter', system-ui, sans-serif" dominantBaseline="central">P</text>
    </svg>
  );
}

export function FanboxIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" style={{ width: "18px", height: "18px", flexShrink: 0 }}>
      <circle cx="12" cy="12" r="10" fill="#F2C624" />
      <text x="50%" y="50%" fill="#FFFFFF" fontSize="12" fontWeight="800" textAnchor="middle" fontFamily="'Inter', system-ui, sans-serif" dominantBaseline="central">F</text>
    </svg>
  );
}

export function XIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" style={{ width: "18px", height: "18px", flexShrink: 0 }}>
      <circle cx="12" cy="12" r="10" fill="#0F172A" />
      <path d="M8 7l8 10M16 7L8 17" stroke="#FFFFFF" strokeWidth="2.2" strokeLinecap="round" />
    </svg>
  );
}

export function CloseIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" style={{ width: "16px", height: "16px", flexShrink: 0, ...style }}>
      <line x1="18" y1="6" x2="6" y2="18" />
      <line x1="6" y1="6" x2="18" y2="18" />
    </svg>
  );
}

export function SkebIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" style={{ width: "18px", height: "18px", flexShrink: 0 }}>
      <circle cx="12" cy="12" r="10" fill="#00A3FF" />
      <text x="50%" y="50%" fill="#FFFFFF" fontSize="12" fontWeight="800" textAnchor="middle" fontFamily="'Inter', system-ui, sans-serif" dominantBaseline="central">S</text>
    </svg>
  );
}

export function SettingsIcon() {
  return (
    <svg className="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83 0 2 2 0 010-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 010-2.83 2 2 0 012.83 0l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 0 2 2 0 010 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
    </svg>
  );
}

export function LibraryIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg 
      className={`nav-icon ${className}`}
      viewBox="0 0 24 24" 
      fill="none" 
      stroke="currentColor" 
      strokeWidth="2" 
      strokeLinecap="round" 
      strokeLinejoin="round"
      style={{ flexShrink: 0, ...style }}
    >
      <line x1="3" x2="21" y1="22" y2="22" />
      <line x1="6" x2="6" y1="18" y2="11" />
      <line x1="10" x2="10" y1="18" y2="11" />
      <line x1="14" x2="14" y1="18" y2="11" />
      <line x1="18" x2="18" y1="18" y2="11" />
      <polygon points="12 2 20 7 4 7" />
    </svg>
  );
}

export function SearchIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="11" cy="11" r="8" />
      <line x1="21" y1="21" x2="16.65" y2="16.65" />
    </svg>
  );
}

export function ChevronLeftIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg className={className} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0, ...style }}>
      <polyline points="15,18 9,12 15,6" />
    </svg>
  );
}

export function ChevronRightIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg className={className} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0, ...style }}>
      <polyline points="9,6 15,12 9,18" />
    </svg>
  );
}

export function RefreshIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg className={className} width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0, ...style }}>
      <polyline points="23,4 23,10 17,10" />
      <path d="M20.49 15a9 9 0 11-2.12-9.36L23 10" />
    </svg>
  );
}

export function DownloadIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4" />
      <polyline points="7,10 12,15 17,10" />
      <line x1="12" y1="15" x2="12" y2="3" />
    </svg>
  );
}

export function TrashIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="3,6 5,6 21,6" />
      <path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2" />
    </svg>
  );
}

export function ExportIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4" />
      <polyline points="17,8 12,3 7,8" />
      <line x1="12" y1="3" x2="12" y2="15" />
    </svg>
  );
}

export function FolderIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
    </svg>
  );
}

export function FileIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg 
      className={className}
      viewBox="0 0 24 24" 
      fill="none" 
      stroke="currentColor" 
      strokeWidth="2" 
      strokeLinecap="round" 
      strokeLinejoin="round"
      style={{ width: '16px', height: '16px', flexShrink: 0, ...style }}
    >
      <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z" />
      <polyline points="14,2 14,8 20,8" />
    </svg>
  );
}

export function ImageIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
      <circle cx="8.5" cy="8.5" r="1.5" />
      <polyline points="21,15 16,10 5,21" />
    </svg>
  );
}

export function AlertIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="10" />
      <line x1="12" y1="8" x2="12" y2="12" />
      <line x1="12" y1="16" x2="12.01" y2="16" />
    </svg>
  );
}

export function ArrowLeftIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="19" y1="12" x2="5" y2="12" />
      <polyline points="12,19 5,12 12,5" />
    </svg>
  );
}

export function GlobeIcon() {
  return (
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="10" />
      <line x1="2" y1="12" x2="22" y2="12" />
      <path d="M12 2a15.3 15.3 0 014 10 15.3 15.3 0 01-4 10 15.3 15.3 0 01-4-10 15.3 15.3 0 014-10z" />
    </svg>
  );
}

export function BookOpenIcon() {
  return (
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M2 3h6a4 4 0 014 4v14a3 3 0 00-3-3H2z" />
      <path d="M22 3h-6a4 4 0 00-4 4v14a3 3 0 013-3h7z" />
    </svg>
  );
}

export function LinkIcon() {
  return (
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M10 13a5 5 0 007.54.54l3-3a5 5 0 00-7.07-7.07l-1.72 1.71" />
      <path d="M14 11a5 5 0 00-7.54-.54l-3 3a5 5 0 007.07 7.07l1.71-1.71" />
    </svg>
  );
}

export function ArchiveIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="21,8 21,21 3,21 3,8" />
      <rect x="1" y="3" width="22" height="5" />
      <line x1="10" y1="12" x2="14" y2="12" />
    </svg>
  );
}

export function SyncIcon({ active, className = "", style }: { active?: boolean; className?: string; style?: React.CSSProperties }) {
  const defaultStyle: React.CSSProperties = {
    transition: "transform 0.4s ease",
    ...style
  };
  
  if (active !== undefined) {
    defaultStyle.color = active ? "#ffffff" : "rgba(255, 255, 255, 0.85)";
  }

  return (
    <svg 
      className={className} 
      width="14" 
      height="14" 
      viewBox="0 0 24 24" 
      fill="none" 
      stroke="currentColor" 
      strokeWidth="2.5" 
      strokeLinecap="round" 
      strokeLinejoin="round"
      style={defaultStyle}
    >
      <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
      <path d="M3 3v5h5" />
      <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
      <path d="M16 16h5v5" />
    </svg>
  );
}

export function TerminalAlertIcon({ className = "" }: { className?: string }) {
  return (
    <svg 
      className={className} 
      width="12" 
      height="12" 
      viewBox="0 0 24 24" 
      fill="none" 
      stroke="currentColor" 
      strokeWidth="2.5" 
      strokeLinecap="round" 
      strokeLinejoin="round"
      style={{ flexShrink: 0 }}
    >
      <circle cx="12" cy="12" r="10" />
      <line x1="12" y1="8" x2="12" y2="12" />
      <line x1="12" y1="16" x2="12.01" y2="16" />
    </svg>
  );
}

export function UpdateOnIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M21 12a9 9 0 11-9-9" />
      <polyline points="9 11 12 14 22 4" />
    </svg>
  );
}

export function UpdateOffIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg 
      className={className} 
      width="14" 
      height="14" 
      viewBox="0 0 24 24" 
      fill="none" 
      stroke="currentColor" 
      strokeWidth="2.5" 
      strokeLinecap="round" 
      strokeLinejoin="round"
      style={{ flexShrink: 0, ...style }}
    >
      <circle cx="12" cy="12" r="10" />
      <line x1="4.93" y1="4.93" x2="19.07" y2="19.07" />
    </svg>
  );
}

export function FunnelIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0 }}>
      <polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3" />
    </svg>
  );
}

export function BookIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg 
      className={`nav-icon ${className}`} 
      viewBox="0 0 24 24" 
      style={{ flexShrink: 0, ...style }}
    >
      {/* 本のベース（表紙は常に緑色 #86B918、輪郭線は灰色 #55595D） */}
      <path 
        d="M6.5 2H20v20H6.5A2.5 2.5 0 014 19.5v-15A2.5 2.5 0 016.5 2z" 
        fill="#86B918" 
        stroke="#55595D" 
        strokeWidth="1.6" 
        strokeLinecap="round" 
        strokeLinejoin="round" 
      />
      {/* 本のページの厚みを表す下の部分（白色 #FFFFFF で塗りつぶし、輪郭線は灰色 #55595D） */}
      <path 
        d="M4 19.5A2.5 2.5 0 016.5 17H20v5H6.5A2.5 2.5 0 014 19.5z" 
        fill="#FFFFFF" 
        stroke="#55595D" 
        strokeWidth="1.6" 
        strokeLinecap="round" 
        strokeLinejoin="round" 
      />
      {/* 中央の白い小文字のe（本の表紙の真ん中に配置するため少し上に調整） */}
      <text 
        x="12.2" 
        y="9.0" 
        fill="#FFFFFF"  
        fontSize="11" 
        fontWeight="800" 
        textAnchor="middle" 
        fontFamily="'Inter', system-ui, sans-serif" 
        dominantBaseline="central"
      >
        e
      </text>
    </svg>
  );
}

export function TemplateIcon() {
  return (
    <svg className="nav-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0 }}>
      <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z" />
      <polyline points="17 21 17 13 7 13 7 21" />
      <polyline points="7 3 7 8 15 8" />
    </svg>
  );
}

export function PlusIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0 }}>
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  );
}

export function SaveIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg 
      className={className}
      viewBox="0 0 24 24" 
      fill="none" 
      stroke="currentColor" 
      strokeWidth="2" 
      strokeLinecap="round" 
      strokeLinejoin="round" 
      style={{ width: '14px', height: '14px', flexShrink: 0, ...style }}
    >
      <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z" />
      <polyline points="17 21 17 13 7 13 7 21" />
      <polyline points="7 3 7 8 15 8" />
    </svg>
  );
}

export function PanelRightIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ width: "16px", height: "16px", flexShrink: 0, ...style }}
    >
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <line x1="15" y1="4" x2="15" y2="20" />
    </svg>
  );
}

export function GalleryIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ width: "16px", height: "16px", flexShrink: 0, ...style }}
    >
      <rect x="3" y="3" width="7" height="7" />
      <rect x="14" y="3" width="7" height="7" />
      <rect x="14" y="14" width="7" height="7" />
      <rect x="3" y="14" width="7" height="7" />
    </svg>
  );
}

export function CompactIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ width: "16px", height: "16px", flexShrink: 0, ...style }}
    >
      <line x1="3" y1="6" x2="21" y2="6" />
      <line x1="3" y1="12" x2="21" y2="12" />
      <line x1="3" y1="18" x2="21" y2="18" />
    </svg>
  );
}

export function CheckSquareIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg className={className} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0, ...style }}>
      <polyline points="9 11 12 14 22 4" />
      <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
    </svg>
  );
}

export function SquareIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg className={className} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0, ...style }}>
      <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
    </svg>
  );
}

export function CheckSquaresIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg className={`nav-icon ${className}`} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ width: "18px", height: "18px", flexShrink: 0, ...style }}>
      <path d="M9 20h9a2 2 0 0 0 2-2V9" />
      <path d="M13 5H7a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-6" />
      <polyline points="9 11 12 14 20 6" />
    </svg>
  );
}

export function SquaresIcon({ className = "", style }: { className?: string; style?: React.CSSProperties } = {}) {
  return (
    <svg className={`nav-icon ${className}`} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ width: "18px", height: "18px", flexShrink: 0, ...style }}>
      <path d="M9 20h9a2 2 0 0 0 2-2V9" />
      <rect x="5" y="5" width="12" height="12" rx="2" ry="2" />
    </svg>
  );
}

