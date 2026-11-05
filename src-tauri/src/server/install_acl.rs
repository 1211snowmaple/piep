//! サーバーの配置先の権限。SYSTEM で動かす前に、管理者以外が中身を差し替えられないことを確かめる。
//!
//! 実行ファイル、DLL、版を選ぶファイルのどれかを他の利用者が書き換えられると、次の起動で
//! SYSTEM としてその人のプログラムが動く。既に中身のあるフォルダーは勝手に権限を変えず、
//! 危なければ断る。空のフォルダーと新しいフォルダーだけ、所有者と権限を置き換える。

use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorDacl,
    GetSecurityDescriptorOwner, GetTokenInformation, TokenUser, ACE_HEADER, ACL,
    ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

const REPARSE_POINT: u32 = 0x400;
const INHERIT_ONLY: u8 = 0x08;
const ACE_ALLOWED: u8 = 0;
const ACE_DENIED: u8 = 1;
const ACE_ALLOWED_CALLBACK: u8 = 9;

const FILE_WRITE_DATA: u32 = 0x0002;
const FILE_APPEND_DATA: u32 = 0x0004;
const FILE_WRITE_EA: u32 = 0x0010;
const FILE_DELETE_CHILD: u32 = 0x0040;
const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// 置き場の中で、他の人に許してはいけない権限。
const CONTENT_RISK: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_WRITE_EA
    | FILE_DELETE_CHILD
    | FILE_WRITE_ATTRIBUTES
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_ALL
    | GENERIC_WRITE;
/// 親で許してはいけない権限。消せる、名前を変えられる、権限を変えられると、置き場ごと差し替えられる。
/// 親にファイルを足せるだけなら、置き場には届かない。
const PARENT_RISK: u32 = FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL;
/// ドライブの直下は名前を変えられないので、DELETE を問わない。
const DRIVE_RISK: u32 = FILE_DELETE_CHILD | WRITE_DAC | WRITE_OWNER | GENERIC_ALL;

#[derive(Debug, Clone, PartialEq, Eq)]
struct AllowEntry {
    sid: String,
    mask: u32,
    inherit_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Security {
    owner: String,
    /// `None` は DACL がない状態で、誰でも何でもできる。
    entries: Option<Vec<AllowEntry>>,
}

fn trusted(sid: &str, installer: &str) -> bool {
    sid == SYSTEM || sid == ADMINISTRATORS || sid == TRUSTED_INSTALLER || sid == installer
}

/// 信頼できない人に危ない権限があれば、それを説明する文を返す。
fn risk(security: &Security, installer: &str, forbidden: u32) -> Option<String> {
    if !trusted(&security.owner, installer) {
        return Some(format!(
            "所有者が管理者ではありません（{}）",
            security.owner
        ));
    }
    let Some(entries) = &security.entries else {
        return Some("権限が設定されておらず、誰でも書き換えられます".into());
    };
    entries
        .iter()
        .find(|entry| {
            !entry.inherit_only && !trusted(&entry.sid, installer) && entry.mask & forbidden != 0
        })
        .map(|entry| format!("管理者以外に書き換えの権限があります（{}）", entry.sid))
}

fn wide_error(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{} の権限を確かめられません: {error}", path.display())
}

fn sid_text(sid: PSID) -> Result<String, String> {
    let mut text = PWSTR::null();
    // SAFETY: sid は呼び出し元が持つ記述子の中を指す。受け取った文字列は LocalFree で返す。
    unsafe {
        ConvertSidToStringSidW(sid, &mut text).map_err(|error| error.to_string())?;
        let result = text.to_string().map_err(|error| error.to_string());
        let _ = LocalFree(Some(HLOCAL(text.0 as *mut _)));
        result
    }
}

/// 許可の項目を読む。読めない種類は、安全のために「信頼できない人の全権限」として扱う。
///
/// # Safety
/// `acl` は有効な ACL を指していること。
unsafe fn read_entries(acl: *const ACL) -> Result<Vec<AllowEntry>, String> {
    let mut info = ACL_SIZE_INFORMATION::default();
    GetAclInformation(
        acl,
        std::ptr::from_mut(&mut info).cast(),
        size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    )
    .map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    for index in 0..info.AceCount {
        let mut ace = std::ptr::null_mut();
        GetAce(acl, index, &mut ace).map_err(|error| error.to_string())?;
        let header = &*ace.cast::<ACE_HEADER>();
        match header.AceType {
            ACE_DENIED => {}
            ACE_ALLOWED | ACE_ALLOWED_CALLBACK => {
                // ACCESS_ALLOWED_ACE: ヘッダー 4 バイト、権限 4 バイト、続けて SID。
                let mask = ace.cast::<u8>().add(4).cast::<u32>().read_unaligned();
                let sid = PSID(ace.cast::<u8>().add(8).cast());
                entries.push(AllowEntry {
                    sid: sid_text(sid)?,
                    mask,
                    inherit_only: header.AceFlags & INHERIT_ONLY != 0,
                });
            }
            _ => entries.push(AllowEntry {
                sid: "不明な種類の項目".into(),
                mask: u32::MAX,
                inherit_only: header.AceFlags & INHERIT_ONLY != 0,
            }),
        }
    }
    Ok(entries)
}

fn read_security(path: &Path) -> Result<Security, String> {
    let name = HSTRING::from(path.as_os_str());
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: 受け取った記述子は、読み終えてから LocalFree で返す。owner と dacl はその中を指す。
    unsafe {
        let status = GetNamedSecurityInfoW(
            &name,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            &mut descriptor,
        );
        if status.is_err() {
            return Err(wide_error(path, status.0));
        }
        let read = (|| {
            Ok(Security {
                owner: sid_text(owner)?,
                entries: if dacl.is_null() {
                    None
                } else {
                    Some(read_entries(dacl)?)
                },
            })
        })();
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        read.map_err(|error: String| wide_error(path, error))
    }
}

/// このプロセスを動かしている利用者の SID。
fn current_user_sid() -> Result<String, String> {
    // SAFETY: 自分のトークンを読むだけで、ハンドルは閉じる。TOKEN_USER はバッファの先頭にある。
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|error| format!("利用者を確かめられません: {error}"))?;
        let mut length = 0;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut length);
        let mut buffer = vec![0_u64; (length as usize).div_ceil(8)];
        let read = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &mut length,
        );
        let _ = CloseHandle(token);
        read.map_err(|error| format!("利用者を確かめられません: {error}"))?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        sid_text(user.User.Sid)
    }
}

fn is_reparse_point(path: &Path) -> Result<bool, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| wide_error(path, error))?;
    Ok(metadata.file_attributes() & REPARSE_POINT != 0)
}

/// 親を順に見る。リンクを挟む道は、先を差し替えられるので断る。
fn check_ancestors(root: &Path, installer: &str) -> Result<(), String> {
    for ancestor in root.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        if is_reparse_point(ancestor)? {
            return Err(format!(
                "{} はリンクなので、置き場の道に使えません",
                ancestor.display()
            ));
        }
        let forbidden = if ancestor.parent().is_some() {
            PARENT_RISK
        } else {
            DRIVE_RISK
        };
        if let Some(problem) = risk(&read_security(ancestor)?, installer, forbidden) {
            return Err(format!("{}: {problem}", ancestor.display()));
        }
    }
    Ok(())
}

/// 実行に関わる場所を調べる。本文の置き場（`library\downloads`）はプログラムを含まず数も多いので歩かない。
fn check_contents(root: &Path, installer: &str) -> Result<(), String> {
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        if is_reparse_point(&path)? {
            return Err(format!("{} はリンクです", path.display()));
        }
        if let Some(problem) = risk(&read_security(&path)?, installer, CONTENT_RISK) {
            return Err(format!("{}: {problem}", path.display()));
        }
        if !path.is_dir() || path.ends_with(Path::new("library").join("downloads")) {
            continue;
        }
        let entries = fs::read_dir(&path).map_err(|error| wide_error(&path, error))?;
        for entry in entries {
            stack.push(entry.map_err(|error| wide_error(&path, error))?.path());
        }
    }
    Ok(())
}

/// 所有者と権限を丸ごと置き換える。他の人に許していた項目は残らない。子には親の権限が伝わる。
fn replace_security(root: &Path, owner: &str, installer: &str) -> Result<(), String> {
    let sddl = HSTRING::from(format!(
        "O:{owner}D:P(A;OICI;FA;;;{SYSTEM})(A;OICI;FA;;;{ADMINISTRATORS})(A;OICI;FA;;;{installer})"
    ));
    let name = HSTRING::from(root.as_os_str());
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: 作った記述子は、適用のあとで LocalFree で返す。owner と dacl はその中を指す。
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &sddl,
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .map_err(|error| wide_error(root, error))?;
        let applied = (|| {
            let mut new_owner = PSID::default();
            let mut defaulted = false.into();
            GetSecurityDescriptorOwner(descriptor, &mut new_owner, &mut defaulted)
                .map_err(|error| error.to_string())?;
            let mut present = false.into();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
                .map_err(|error| error.to_string())?;
            let status = SetNamedSecurityInfoW(
                &name,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                Some(new_owner),
                None,
                Some(dacl),
                None,
            );
            if status.is_err() {
                return Err(format!("権限を設定できません（{}）", status.0));
            }
            Ok(())
        })();
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        applied.map_err(|error: String| wide_error(root, error))
    }
}

fn is_empty_directory(root: &Path) -> Result<bool, String> {
    Ok(fs::read_dir(root)
        .map_err(|error| wide_error(root, error))?
        .next()
        .is_none())
}

fn prepare(root: &Path, owner: Option<&str>, installer: &str) -> Result<(), String> {
    if !root.exists() {
        fs::create_dir_all(root).map_err(|error| format!("置き場を作れません: {error}"))?;
    }
    check_ancestors(root, installer)?;
    prepare_folder(root, owner, installer)
}

/// 空なら権限を置き換え、中身があれば権限を変えずに調べる。
fn prepare_folder(root: &Path, owner: Option<&str>, installer: &str) -> Result<(), String> {
    if !root.is_dir() {
        return Err(format!("{} はフォルダーではありません", root.display()));
    }
    if is_reparse_point(root)? {
        return Err(format!("{} はリンクです", root.display()));
    }
    if is_empty_directory(root)? {
        replace_security(root, owner.unwrap_or(installer), installer)?;
    }
    check_contents(root, installer)
}

/// SYSTEM のタスクを登録する前に呼ぶ。通らなければ、タスクも実行ファイルも置かない。
pub fn secure_install_root(root: &Path) -> Result<(), String> {
    let installer = current_user_sid()?;
    prepare(root, Some(ADMINISTRATORS), &installer).map_err(|problem| {
        format!(
            "この場所では SYSTEM として動かせません。{problem}\n別のフォルダーを指定するか、管理者だけが書き換えられるように直してから入れ直してください。"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const INSTALLER: &str = "S-1-5-21-1-2-3-1001";
    const OTHER: &str = "S-1-5-21-1-2-3-1002";

    fn security(owner: &str, entries: &[(&str, u32, bool)]) -> Security {
        Security {
            owner: owner.to_string(),
            entries: Some(
                entries
                    .iter()
                    .map(|(sid, mask, inherit_only)| AllowEntry {
                        sid: (*sid).to_string(),
                        mask: *mask,
                        inherit_only: *inherit_only,
                    })
                    .collect(),
            ),
        }
    }

    #[test]
    fn only_admins_and_the_installer_may_write() {
        let safe = security(
            ADMINISTRATORS,
            &[
                (SYSTEM, GENERIC_ALL, false),
                (ADMINISTRATORS, GENERIC_ALL, false),
                (INSTALLER, GENERIC_ALL, false),
                // 読むだけ、子にだけ効く項目は問題にしない。
                ("S-1-5-32-545", 0x0012_00A9, false),
                ("S-1-3-0", GENERIC_ALL, true),
            ],
        );
        assert_eq!(risk(&safe, INSTALLER, CONTENT_RISK), None);
        assert!(risk(&security(OTHER, &[]), INSTALLER, CONTENT_RISK).is_some());
        assert!(risk(
            &security(ADMINISTRATORS, &[(OTHER, FILE_WRITE_DATA, false)]),
            INSTALLER,
            CONTENT_RISK
        )
        .is_some());
        assert!(risk(
            &Security {
                owner: ADMINISTRATORS.into(),
                entries: None
            },
            INSTALLER,
            CONTENT_RISK
        )
        .is_some());
    }

    #[test]
    fn a_parent_may_let_others_add_but_not_replace() {
        // ProgramData と同じ。Users はファイルを足せるが、消せない。
        let adds_only = security(
            SYSTEM,
            &[("S-1-5-32-545", FILE_WRITE_DATA | FILE_APPEND_DATA, false)],
        );
        assert_eq!(risk(&adds_only, INSTALLER, PARENT_RISK), None);
        for mask in [
            FILE_DELETE_CHILD,
            DELETE,
            WRITE_DAC,
            WRITE_OWNER,
            GENERIC_ALL,
        ] {
            let replaceable = security(SYSTEM, &[(OTHER, mask, false)]);
            assert!(
                risk(&replaceable, INSTALLER, PARENT_RISK).is_some(),
                "{mask}"
            );
        }
        // ドライブの直下の Modify は DELETE を含むが、名前は変えられない。
        let drive = security(SYSTEM, &[("S-1-5-11", DELETE | FILE_WRITE_DATA, false)]);
        assert_eq!(risk(&drive, INSTALLER, DRIVE_RISK), None);
    }

    fn icacls(path: &Path, args: &[&str]) {
        let status = Command::new("icacls")
            .arg(path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "piep-acl-{name}-{}-{}",
            std::process::id(),
            crate::server::store::random_token(4)
        ));
        fs::create_dir_all(&path).unwrap();
        // 一時フォルダーには環境ごとの他の権限が付いていることがあるので、自分だけにして始める。
        let installer = current_user_sid().unwrap();
        replace_security(&path, &installer, &installer).unwrap();
        path
    }

    #[test]
    fn a_folder_someone_else_can_write_is_refused_and_left_alone() {
        let installer = current_user_sid().unwrap();
        let root = scratch("existing");
        fs::write(root.join("piep-server.exe"), b"x").unwrap();
        // 中身があるフォルダーの権限は変えず、断る。
        icacls(&root, &["/grant", "*S-1-5-11:(OI)(CI)M"]);
        let error = check_contents(&root, &installer).unwrap_err();
        assert!(error.contains("S-1-5-11"), "{error}");
        let error = prepare_folder(&root, None, &installer).unwrap_err();
        assert!(error.contains("S-1-5-11"), "{error}");
        let still = read_security(&root).unwrap();
        assert!(still
            .entries
            .unwrap()
            .iter()
            .any(|entry| entry.sid == "S-1-5-11"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_explicit_grant_deep_inside_is_found() {
        let installer = current_user_sid().unwrap();
        let root = scratch("deep");
        let versions = root.join("versions").join("0.35.0");
        fs::create_dir_all(&versions).unwrap();
        let program = versions.join("piep-server.exe");
        fs::write(&program, b"x").unwrap();
        check_contents(&root, &installer).unwrap();
        icacls(&program, &["/grant", "*S-1-5-11:M"]);
        assert!(check_contents(&root, &installer).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_empty_folder_is_taken_over_without_leftover_grants() {
        let installer = current_user_sid().unwrap();
        let root = scratch("empty");
        icacls(&root, &["/grant", "*S-1-5-11:(OI)(CI)M"]);
        prepare_folder(&root, None, &installer).unwrap();
        let security = read_security(&root).unwrap();
        assert_eq!(security.owner, installer);
        assert!(security
            .entries
            .as_ref()
            .unwrap()
            .iter()
            .all(|entry| entry.sid != "S-1-5-11"));
        fs::write(root.join("state.db"), b"x").unwrap();
        check_contents(&root, &installer).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_parent_that_others_can_replace_is_refused() {
        let installer = current_user_sid().unwrap();
        let parent = scratch("parent");
        let root = parent.join("piep-server");
        fs::create_dir_all(&root).unwrap();
        icacls(&parent, &["/grant", "*S-1-5-11:(DE)"]);
        let error = read_security(&parent)
            .map(|security| risk(&security, &installer, PARENT_RISK))
            .unwrap()
            .unwrap();
        assert!(error.contains("S-1-5-11"), "{error}");
        fs::remove_dir_all(&parent).unwrap();
    }

    #[test]
    fn a_link_in_the_folder_is_refused() {
        let installer = current_user_sid().unwrap();
        let root = scratch("link");
        let target = scratch("link-target");
        let link = root.join("versions");
        let made = Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success());
        let error = check_contents(&root, &installer).unwrap_err();
        assert!(error.contains("リンク"), "{error}");
        fs::remove_dir(&link).unwrap();
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&target).unwrap();
    }
}
