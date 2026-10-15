// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn startup_error_message(error: &dyn std::fmt::Display) -> String {
    format!("piepを起動できませんでした。\n\n{error}").replace('\0', "�")
}

#[cfg(windows)]
fn report_startup_error(message: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TASKMODAL,
    };

    let message: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "piep - 起動エラー"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: both UTF-16 buffers are NUL-terminated and remain alive for the
    // duration of this synchronous Win32 call. A null owner is intentional
    // because the Tauri window does not exist when setup fails.
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TASKMODAL,
        );
    }
}

#[cfg(not(windows))]
fn report_startup_error(message: &str) {
    eprintln!("{message}");
}

fn main() {
    if let Err(error) = piep_lib::run() {
        report_startup_error(&startup_error_message(&error));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_error_is_visible_actionable_japanese_and_nul_safe() {
        let message = startup_error_message(&"別のpiepを閉じてから再起動してください。\0hidden");
        assert!(message.starts_with("piepを起動できませんでした。"));
        assert!(message.contains("別のpiepを閉じてから再起動してください。"));
        assert!(!message.contains('\0'));
    }
}
