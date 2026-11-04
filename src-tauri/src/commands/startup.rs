//! 起動の順番と、最初に見える窓の色。
//!
//! Tauri は窓を作ったあとにメインスレッドで `setup` を呼ぶ。ここで棚を開くと、
//! 終わるまで窓のメッセージループが止まる。ページも読み込めないので、色だけの窓が
//! 固まり、数秒で Windows が「応答なし」を付けて白く塗る。棚の準備は別のスレッドで
//! 進め、画面には準備が済むのを待ってもらう。

use std::path::Path;
use std::sync::Mutex;
use tauri::window::Color;
use tauri::{Manager, State};
use tokio::sync::watch;

/// 窓の色を決めるために残す、配色の設定。
///
/// 画面側の設定は WebView の localStorage にあり、窓を作る時点では読めない。
const WINDOW_SCHEME_FILENAME: &str = "window-color-scheme";

// `src/styles/app.css` の body と `public/startup.css` に合わせる。
const LIGHT_BACKGROUND: Color = Color(0xF5, 0xF7, 0xFA, 0xFF);
const DARK_BACKGROUND: Color = Color(0x0D, 0x11, 0x17, 0xFF);

/// 前回の配色の設定から、窓の背景色を決める。
pub fn window_background(app_data: &Path) -> Color {
    let saved = std::fs::read_to_string(app_data.join(WINDOW_SCHEME_FILENAME)).ok();
    background_for(saved.as_deref(), os_prefers_dark)
}

fn background_for(saved: Option<&str>, os_prefers_dark: impl FnOnce() -> bool) -> Color {
    let dark = match saved.map(str::trim) {
        Some("dark") => true,
        Some("light") => false,
        // 「システムに合わせる」と未保存。OS の設定は前回から変わっているかもしれない
        // ので、解決済みの色ではなく設定そのものを残して、毎回ここで読む。
        _ => os_prefers_dark(),
    };
    if dark {
        DARK_BACKGROUND
    } else {
        LIGHT_BACKGROUND
    }
}

#[cfg(windows)]
fn os_prefers_dark() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let mut uses_light = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both pointers refer to live locals, and `size` states the buffer's
    // length. RRF_RT_REG_DWORD rejects any value that is not a 4-byte DWORD.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut uses_light).cast()),
            Some(&mut size),
        )
    };
    status.is_ok() && uses_light == 0
}

#[cfg(not(windows))]
fn os_prefers_dark() -> bool {
    false
}

/// 棚の準備が終わったかどうかを、画面へ伝える。
#[derive(Default)]
pub struct StartupGate {
    state: watch::Sender<Option<Result<(), String>>>,
}

impl StartupGate {
    pub fn finish(&self, result: Result<(), String>) {
        self.state.send_replace(Some(result));
    }

    async fn wait(&self) -> Result<(), String> {
        let mut receiver = self.state.subscribe();
        let finished = receiver
            .wait_for(Option::is_some)
            .await
            .map_err(|error| error.to_string())?;
        finished.clone().unwrap_or(Ok(()))
    }
}

/// 窓の色の控えを書くときに、同じ起動の中で順番が入れ替わらないようにする。
#[derive(Default)]
pub struct WindowSchemeFile {
    write: Mutex<()>,
}

/// 棚の準備が終わるまで待つ。画面はこれを待ってから他のコマンドを呼ぶ。
///
/// `AppState` は準備が済んでから登録されるので、先に呼ばれたコマンドは状態を
/// 取り出せずに落ちる。**これより前にコマンドを呼ばせない。**
#[tauri::command]
pub async fn startup_wait_for_library(gate: State<'_, StartupGate>) -> Result<(), String> {
    gate.wait().await
}

/// 配色の設定を、次の起動で窓の色に使えるように残す。
#[tauri::command]
pub fn startup_remember_color_scheme(
    app: tauri::AppHandle,
    file: State<'_, WindowSchemeFile>,
    scheme: String,
) -> Result<(), String> {
    if !matches!(scheme.as_str(), "light" | "dark" | "auto") {
        return Err(format!("知らない配色です: {scheme}"));
    }
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let _write = file.write.lock().map_err(|e| e.to_string())?;
    let path = app_data.join(WINDOW_SCHEME_FILENAME);
    if std::fs::read_to_string(&path).is_ok_and(|saved| saved == scheme) {
        return Ok(());
    }
    std::fs::write(path, scheme).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_scheme_wins_over_the_os_setting() {
        assert_eq!(background_for(Some("dark"), || false), DARK_BACKGROUND);
        assert_eq!(background_for(Some("light"), || true), LIGHT_BACKGROUND);
    }

    #[test]
    fn following_the_os_reads_it_at_each_launch() {
        for saved in [None, Some("auto"), Some("unknown")] {
            assert_eq!(background_for(saved, || true), DARK_BACKGROUND);
            assert_eq!(background_for(saved, || false), LIGHT_BACKGROUND);
        }
    }

    #[tokio::test]
    async fn the_gate_answers_callers_from_before_and_after_it_finishes() {
        let gate = std::sync::Arc::new(StartupGate::default());
        let early = {
            let gate = std::sync::Arc::clone(&gate);
            tokio::spawn(async move { gate.wait().await })
        };
        gate.finish(Err("開けません".to_string()));
        assert_eq!(early.await.unwrap(), Err("開けません".to_string()));
        assert_eq!(gate.wait().await, Err("開けません".to_string()));
    }
}
