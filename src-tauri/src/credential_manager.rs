//! Windows の資格情報マネージャーへの薄い入口。
//!
//! 鍵（サーバーの管理者の鍵、pixiv と FANBOX の接続）はここにだけ置く。
//! 設定のファイルは平文で、誰でも読める場所にあるので、鍵を書かない。
//!
//! 名前は `io.github.u1211snowmaple.piep/` で始める。資格情報マネージャーの
//! 一覧で、どのアプリのものか分かるように。
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

/// すべての名前の頭。
pub const PREFIX: &str = "io.github.u1211snowmaple.piep";

/// 一つの資格情報に置ける大きさの上限（`CRED_MAX_CREDENTIAL_BLOB_SIZE`）。
pub const MAX_BLOB: usize = 5 * 512;

const NOT_FOUND_HRESULT: i32 = 0x8007_0490_u32 as i32;

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `name` に `value` を置く。同じ名前のものは置き換わる。
pub fn write(name: &str, value: &[u8]) -> Result<(), String> {
    if value.len() > MAX_BLOB {
        return Err("資格情報マネージャーに置ける大きさを超えています".into());
    }
    let mut target = wide(name);
    let mut blob = value.to_vec();
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    unsafe { CredWriteW(&credential, 0) }
        .map_err(|_| "資格情報マネージャーに保存できません".to_string())
}

/// `name` に置いたものを読む。無ければ `None`。
pub fn read(name: &str) -> Result<Option<Vec<u8>>, String> {
    let target = wide(name);
    let mut pointer: *mut CREDENTIALW = std::ptr::null_mut();
    if let Err(error) = unsafe {
        CredReadW(
            PCWSTR(target.as_ptr()),
            CRED_TYPE_GENERIC,
            None,
            &mut pointer,
        )
    } {
        if error.code().0 == NOT_FOUND_HRESULT {
            return Ok(None);
        }
        return Err("資格情報マネージャーから読めません".into());
    }
    // CredReadW が確保した領域は、中身を読めなくても必ず CredFree する。
    let value = unsafe {
        let credential = &*pointer;
        if credential.CredentialBlob.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
            .to_vec()
        }
    };
    unsafe { CredFree(pointer as *const _) };
    Ok(Some(value))
}

/// 共通の接頭辞を持つ汎用資格情報の名前を列挙する。
pub fn names_with_prefix(prefix: &str) -> Result<Vec<String>, String> {
    let filter = wide(&format!("{prefix}*"));
    let mut count = 0;
    let mut pointer: *mut *mut CREDENTIALW = std::ptr::null_mut();
    if let Err(error) =
        unsafe { CredEnumerateW(PCWSTR(filter.as_ptr()), None, &mut count, &mut pointer) }
    {
        if error.code().0 == NOT_FOUND_HRESULT {
            return Ok(Vec::new());
        }
        return Err("資格情報マネージャーの一覧を読めません".into());
    }
    if pointer.is_null() {
        return Ok(Vec::new());
    }
    // 列挙結果は名前だけを取り出し、まとめて解放する。
    let names = unsafe {
        let names = std::slice::from_raw_parts(pointer, count as usize)
            .iter()
            .map(|&entry| &*entry)
            .filter(|entry| entry.Type == CRED_TYPE_GENERIC)
            .map(|entry| entry.TargetName.to_string())
            .collect::<Result<Vec<_>, _>>();
        CredFree(pointer as *const _);
        names.map_err(|_| "資格情報の名前を読めません".to_string())?
    };
    Ok(names)
}

/// `name` を消す。もともと無くてもよい。
pub fn delete(name: &str) -> Result<(), String> {
    let target = wide(name);
    match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(()),
        Err(error) if error.code().0 == NOT_FOUND_HRESULT => Ok(()),
        Err(_) => Err("資格情報マネージャーから消せません".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_round_trips_and_can_be_removed() {
        let name = format!("{PREFIX}/test/{}", uuid::Uuid::now_v7());
        assert_eq!(read(&name).unwrap(), None);
        assert!(names_with_prefix(&name).unwrap().is_empty());
        write(&name, b"value").unwrap();
        assert_eq!(names_with_prefix(&name).unwrap(), vec![name.clone()]);
        assert_eq!(read(&name).unwrap().as_deref(), Some(&b"value"[..]));
        write(&name, b"other").unwrap();
        assert_eq!(read(&name).unwrap().as_deref(), Some(&b"other"[..]));
        delete(&name).unwrap();
        assert_eq!(read(&name).unwrap(), None);
        assert!(names_with_prefix(&name).unwrap().is_empty());
        delete(&name).unwrap();
    }

    #[test]
    fn too_large_a_value_is_refused_before_writing() {
        let name = format!("{PREFIX}/test/{}", uuid::Uuid::now_v7());
        assert!(write(&name, &vec![b'a'; MAX_BLOB + 1]).is_err());
        assert_eq!(read(&name).unwrap(), None);
    }
}
