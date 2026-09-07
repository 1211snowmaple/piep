//! Give native smoke tests their own Tauri paths, including Store and WebView2.
//! Windows Known Folders do not follow APPDATA/LOCALAPPDATA environment changes.

pub const RUN_ID_ENV: &str = "PIEP_SMOKE_TEST_ID";

pub fn configure(
    config: &mut tauri::utils::config::Config,
    run_id: Option<&str>,
) -> Result<(), std::io::Error> {
    let Some(run_id) = run_id else {
        return Ok(());
    };
    if run_id.len() != 32 || !run_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PIEP_SMOKE_TEST_ID must be a 32-character hexadecimal run ID",
        ));
    }
    config.identifier = format!(
        "{}.smoke.{}",
        config.identifier,
        run_id.to_ascii_lowercase()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped_config() -> tauri::utils::config::Config {
        tauri::utils::config::Config {
            identifier: "com.hiron.piep".into(),
            ..Default::default()
        }
    }

    #[test]
    fn normal_launch_keeps_its_identifier() {
        let mut config = shipped_config();
        configure(&mut config, None).unwrap();
        assert_eq!(config.identifier, "com.hiron.piep");
    }

    #[test]
    fn smoke_launch_uses_a_distinct_identifier_and_rejects_paths() {
        let mut config = shipped_config();
        for invalid in [
            "",
            "../piep",
            "com.hiron.piep",
            "0123456789abcdef0123456789abcdeg",
        ] {
            assert!(configure(&mut config, Some(invalid)).is_err());
            assert_eq!(config.identifier, "com.hiron.piep");
        }
        configure(&mut config, Some("0123456789ABCDEF0123456789ABCDEF")).unwrap();
        assert_eq!(
            config.identifier,
            "com.hiron.piep.smoke.0123456789abcdef0123456789abcdef"
        );
    }
}
