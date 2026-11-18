use crate::library::protocol::Id;
use crate::server::store::ServerStore;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

/// 認証できた端末と、その端末が読む利用者。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// 同時に読める数を端末ごとに数えるための識別。
    pub device: String,
    pub subject: Id,
}

/// 認証済み端末を棚と利用者へ明示的に結ぶ。利用者 ID を端末 ID から作らない。
pub trait Authority: Send + Sync {
    fn grant(&self, bearer: &str, library: Id) -> Option<Grant>;
    /// 端末に許した利用者（旧 API で届いた変更を、同じ利用者へ当てるため）。
    fn subject_of(&self, _device: &str, _library: Id) -> Option<Id> {
        None
    }
}

/// 既存の bearer 検証を使うアダプター。grants は管理側が明示して投入する。
/// 旧登録へ自動で新ライブラリの権限を追加しない。鍵はここへ保持しない。
pub struct DeviceAuthority {
    store: Arc<ServerStore>,
    grants: RwLock<HashMap<(String, uuid::Uuid), Id>>,
}
impl DeviceAuthority {
    pub fn new(store: Arc<ServerStore>) -> Self {
        Self {
            store,
            grants: RwLock::new(HashMap::new()),
        }
    }
    pub fn grant(&self, device: String, library: Id, subject: Id) -> Result<(), String> {
        if device.is_empty() {
            return Err("端末の識別が空です".into());
        }
        self.grants
            .write()
            .map_err(|_| "権限の状態を更新できません")?
            .insert((device, library.get()), subject);
        Ok(())
    }
    pub fn revoke(&self, device: &str) -> Result<(), String> {
        self.grants
            .write()
            .map_err(|_| "権限の状態を更新できません")?
            .retain(|(id, _), _| id != device);
        Ok(())
    }
}
impl Authority for DeviceAuthority {
    fn grant(&self, bearer: &str, library: Id) -> Option<Grant> {
        let device = self.store.authorize(bearer)?;
        let subject = *self
            .grants
            .read()
            .ok()?
            .get(&(device.id.clone(), library.get()))?;
        Some(Grant {
            device: device.id,
            subject,
        })
    }
    fn subject_of(&self, device: &str, library: Id) -> Option<Id> {
        self.grants
            .read()
            .ok()?
            .get(&(device.to_owned(), library.get()))
            .copied()
    }
}
