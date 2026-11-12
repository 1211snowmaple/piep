//! サーバーの認証鍵を、ファイルのまま置かないための封。
//!
//! Android では端末の外へ出せない鍵（Keystore）で封じる。利用者の認証は条件にしないので、
//! アプリを開くたびに何かを求めることはない。

pub trait SecretCipher: Send + Sync {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, ()>;
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, ()>;
}

/// デスクトップでの開発用。配布するのは Android 版だけなので、配布物では使わない。
#[cfg(not(target_os = "android"))]
pub struct DevelopmentCipher;

#[cfg(not(target_os = "android"))]
impl SecretCipher for DevelopmentCipher {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, ()> {
        Ok(plain.to_vec())
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, ()> {
        Ok(sealed.to_vec())
    }
}

#[cfg(target_os = "android")]
pub use android::{init, KeystoreCipher};

#[cfg(target_os = "android")]
mod android {
    use super::SecretCipher;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Serialize};
    use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
    use tauri::{Manager, Runtime};

    #[derive(Serialize)]
    struct Payload {
        data: String,
    }

    #[derive(Deserialize)]
    struct Reply {
        data: String,
    }

    pub struct KeystoreCipher<R: Runtime>(PluginHandle<R>);

    impl<R: Runtime> KeystoreCipher<R> {
        fn call(&self, command: &str, input: &[u8]) -> Result<Vec<u8>, ()> {
            let reply: Reply = self
                .0
                .run_mobile_plugin(
                    command,
                    Payload {
                        data: STANDARD.encode(input),
                    },
                )
                .map_err(|_| ())?;
            STANDARD.decode(reply.data).map_err(|_| ())
        }
    }

    impl<R: Runtime> SecretCipher for KeystoreCipher<R> {
        fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, ()> {
            self.call("seal", plain)
        }
        fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, ()> {
            self.call("open", sealed)
        }
    }

    pub fn init<R: Runtime>() -> TauriPlugin<R> {
        Builder::new("secret-cipher")
            .setup(|app, api| {
                let handle = api.register_android_plugin(
                    "io.github.u1211snowmaple.piep.android",
                    "SecretCipherPlugin",
                )?;
                app.manage(KeystoreCipher(handle));
                Ok(())
            })
            .build()
    }

    impl<R: Runtime> Clone for KeystoreCipher<R> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }
}
