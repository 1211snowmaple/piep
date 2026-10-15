; インストーラーに出す絵を、縦横比を保ったまま置く。
;
; MUI2 の既定は `FitControl` で、**縦横比を無視して枠いっぱいに引き伸ばす**
; （`Contrib/Modern UI 2/Interface.nsh` と `Pages.nsh` の `MUI_DEFAULT`）。
; 枠の寸法がこちらの絵と同じなら実害は無いが、Tauri のインストーラーは
;
;     ManifestDPIAware true
;     ManifestDPIAwareness PerMonitorV2
;
; を宣言していて、拡大表示の機械では枠がダイアログ単位から画素へ展開される。
; 横は平均文字幅、縦は文字高で伸びるので**同じ倍率にならない**。結果として
; 150x57 の帯や 164x314 の板が、少しだけ横に伸びる。
;
; `AspectFitHeight` は高さに合わせて等倍で拡げる。幅が余れば地が見え、
; はみ出せば端が切れるが、**ロゴの形は変わらない**。形が崩れるより良い。
;
; ここは Tauri のひな型が `!include MUI2.nsh` の直後、ページを組み立てる前に
; 読み込む唯一の場所である（`installer.nsi` の 35 行目付近）。MUI は
; `MUI_DEFAULT` で「まだ定義されていなければ」入れるので、先に定義すれば勝つ。
; **ひな型ごと差し替えなくて済むのはこの一点による。**
;
; `MUI_HEADERIMAGE_UNBITMAP_STRETCH` は `MUI_HEADERIMAGE_BITMAP_STRETCH` から
; 既定を取るので、アンインストーラー側は指定しなくても揃う。

!define MUI_HEADERIMAGE_BITMAP_STRETCH "AspectFitHeight"
!define MUI_WELCOMEFINISHPAGE_BITMAP_STRETCH "AspectFitHeight"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP_STRETCH "AspectFitHeight"
