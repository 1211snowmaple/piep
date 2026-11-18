//! 編集を反映・取り消した作品の挿絵を見直し、要確認の候補へ反映する。
use crate::library::{Error, Library, LibraryId, Result};
use serde_json::{json, Value};

impl Library {
    pub(crate) fn audit_pixiv_edit_images(&self, work: LibraryId) -> Result<()> {
        if self.library_item_generation(work)?.is_none() {
            return Ok(());
        }
        let card = self.work_card(work)?;
        if card.provider != "pixiv" || card.content_type != "novel" {
            return Ok(());
        }
        let candidate = self.discovery("pixiv", &card.external_key)?;
        let mut payload = json!({
            "kind":"revision", "source":"pixiv", "sourceId":card.external_key,
            "holdKind":"incomplete", "originalData":{"localVersion":card.version_number}
        });
        if let Some(candidate) = &candidate {
            if candidate.decision != "held" || candidate.hold_kind.as_deref() != Some("incomplete")
            {
                return Ok(());
            }
            // 画像だけの保留と確認できる場合に限る。作品情報も伏せられた保留は取得で解く。
            let input: Value = serde_json::from_str(&self.discovery_payload(candidate.id)?)?;
            let reason = input
                .get("holdReason")
                .and_then(Value::as_str)
                .unwrap_or("");
            let has_images = input
                .get("missingReferences")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
                || reason.contains("参照先の挿絵を閲覧できません")
                || reason.contains("挿絵参照を表示できません");
            let has_metadata = input
                .get("missingMetadata")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
                || reason.contains("作品情報");
            if !has_images || has_metadata {
                return Ok(());
            }
            payload = input;
        }
        let snapshot = card.snapshot_id;
        let before = self.edit_state(work, self.id)?;
        let missing = match self.pixiv_source_image_gaps(work, snapshot) {
            Ok(missing) => missing,
            Err(Error::ResourceUnavailable) => return Ok(()),
            Err(error) => return Err(error),
        };
        if missing.is_empty() {
            return Ok(());
        }
        let missing = self.pixiv_references_left_by_active_edit(work, snapshot, missing)?;
        let original = self
            .work_original(work, snapshot)?
            .map(|text| serde_json::from_str::<Value>(&text))
            .transpose()?;
        let masked = original
            .as_ref()
            .map(crate::library::masked_pixiv_metadata)
            .unwrap_or_default();
        let mut reasons = Vec::new();
        if !missing.is_empty() {
            reasons.push(format!("挿絵参照を表示できません: {}", missing.join(", ")));
        }
        if !masked.is_empty() {
            let label = crate::database::queries::masked_metadata_label(&masked);
            reasons.push(format!("作品情報（{label}）が取得元で伏せられた状態です"));
        }
        payload["holdReason"] = json!(format!(
            "[一部保存] 本文は保存済みですが、{}",
            reasons.join("。")
        ));
        payload["missingReferences"] = json!(missing);
        payload["missingMetadata"] = json!(masked);
        self.reconcile_pixiv_image_hold(
            work,
            snapshot,
            before.generation,
            candidate.as_ref(),
            (!reasons.is_empty()).then_some((card.title.as_str(), &payload)),
        )
    }
}
