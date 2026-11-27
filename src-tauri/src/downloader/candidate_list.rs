use std::collections::HashSet;
use std::hash::Hash;

pub(crate) const CANDIDATE_LIMIT: usize = 10_000;

#[derive(Debug, serde::Serialize)]
pub struct CandidateList<T> {
    pub items: Vec<T>,
    pub truncated: bool,
}

impl<T> Default for CandidateList<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            truncated: false,
        }
    }
}

impl<T> CandidateList<T> {
    /// 既出の作品は数えず、確認位置か件数上限で読み取りを止める。
    pub(crate) fn append_page<Id: Eq + Hash>(
        &mut self,
        page: Vec<T>,
        seen: &mut HashSet<Id>,
        id_of: impl Fn(&T) -> Id,
        at_checkpoint: impl Fn(&Id) -> bool,
    ) -> bool {
        for item in page {
            let id = id_of(&item);
            if at_checkpoint(&id) {
                return true;
            }
            if !seen.insert(id) {
                continue;
            }
            if self.items.len() == CANDIDATE_LIMIT {
                self.truncated = true;
                return true;
            }
            self.items.push(item);
        }
        false
    }

    /// 更新確認では、途中までの候補で確認位置を進めない。
    pub(crate) fn into_complete(self) -> Result<Vec<T>, String> {
        if self.truncated {
            return Err(format!(
                "更新確認の候補が {CANDIDATE_LIMIT} 件の上限を超えました"
            ));
        }
        Ok(self.items)
    }
}
