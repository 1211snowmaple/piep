use super::{
    rows::{mapped_id, mark_row, mark_table, marked, LegacyRow},
    LegacyImporter,
};
use crate::library::{
    repository::PreparedFile, EntityKind, Error, Library, Result, SuggestionJson,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Member {
    source: String,
    source_id: String,
    download_id: Option<i64>,
    title: String,
    author_name: String,
    cover_path: Option<String>,
    text_length: i64,
    proposed_position: i64,
    #[serde(deserialize_with = "deserialize_score")]
    score: f64,
    selected: bool,
    evidence: Vec<SuggestionJson>,
}

fn deserialize_score<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> std::result::Result<f64, D::Error> {
    let raw = Box::<serde_json::value::RawValue>::deserialize(de)?;
    raw.get().parse().map_err(serde::de::Error::custom)
}

struct PreparedSuggestion {
    row: LegacyRow,
    seeds: Vec<i64>,
    members: Vec<(Member, Option<PreparedPreview>)>,
    names: Vec<SuggestionJson>,
}

struct PreparedPreview {
    file: PreparedFile,
    name: String,
    path: std::path::PathBuf,
}

impl LegacyImporter {
    pub(super) fn normalize_suggestions(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut suggestions = Vec::new();
        self.each_row("collection_suggestions", |row| {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let seeds: Vec<i64> = serde_json::from_str(row.text("seed_json")?)?;
            let members: Vec<Member> = serde_json::from_str(row.text("members_json")?)?;
            let names: Vec<SuggestionJson> = row
                .optional_text("name_options_json")?
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or_default();
            let mut prepared = Vec::new();
            for member in members {
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::Canceled);
                }
                if member.source.is_empty() || member.source_id.is_empty() {
                    return Err(Error::Integrity("提案内の作品の識別が空です".into()));
                }
                if !member.score.is_finite() || !(0.0..=1.0).contains(&member.score) {
                    return Err(Error::Integrity("提案の作品スコアが不正です".into()));
                }
                let preview = member
                    .cover_path
                    .as_deref()
                    .filter(|p| !p.is_empty())
                    .map(|stored| {
                        let path = self.managed_path(stored)?;
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .ok_or_else(|| Error::UnsafePath(path.clone()))?
                            .to_owned();
                        Ok::<_, Error>(PreparedPreview {
                            file: target.store_path(&path)?,
                            name,
                            path,
                        })
                    })
                    .transpose()?;
                prepared.push((member, preview));
            }
            suggestions.push(PreparedSuggestion {
                row,
                seeds,
                members: prepared,
                names,
            });
            Ok(())
        })?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        // プレビューの圧縮中に他の処理が完了していても、判断を二重適用しない。
        if marked(&tx, "collection_suggestions")? {
            for suggestion in suggestions {
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::Canceled);
                }
                let old = suggestion.row.text("id")?;
                for (member, preview) in suggestion.members {
                    if let Some(preview) = preview {
                        let stored:Vec<u8>=tx.query_row("SELECT m.preview_file_id FROM collection_suggestion_members m JOIN migration_records r ON r.target_id=m.suggestion_id JOIN external_identities e ON e.entity_id=m.work_id WHERE r.source_table='collection_suggestions' AND r.source_key=?1 AND r.status='imported' AND e.provider=?2 AND e.kind='work' AND e.external_key=?3",params![old,member.source,member.source_id],|r|r.get(0))?;
                        if stored != preview.file.file.id.0 {
                            return Err(Error::Integrity(
                                "移行先の提案プレビューが元ファイルと一致しません".into(),
                            ));
                        }
                        self.mark_used_file(&tx, &preview.path, &stored)?;
                    }
                }
            }
            tx.commit()?;
            return Ok(());
        }
        for suggestion in suggestions {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let row = suggestion.row;
            let old = row.text("id")?;
            let id = mapped_id(&tx, "collection_suggestions", old)?;
            let ordered = match row.text("collection_kind")? {
                "ordered" => true,
                "unordered" => false,
                _ => return Err(Error::Integrity("提案の並び方が不正です".into())),
            };
            tx.execute("INSERT INTO collection_suggestions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,1)",params![id.as_bytes().as_slice(),row.text("proposed_name")?,ordered,row.optional_text("track")?.unwrap_or("sequence"),row.optional_text("origin")?.unwrap_or("seed"),row.text("state")?,row.0.get("score").and_then(Value::as_f64).ok_or_else(||Error::Integrity("提案のスコアが不正です".into()))?,row.text("rule_version")?,row.optional_text("evidence_summary")?,serde_json::to_string(&suggestion.names)?,row.time("created_at")?,row.time("updated_at")?])?;
            for (position, seed) in suggestion.seeds.into_iter().enumerate() {
                let work = self.work_from_old(&tx, seed)?;
                tx.execute(
                    "INSERT INTO collection_suggestion_seeds VALUES(?1,?2,?3)",
                    params![
                        id.as_bytes().as_slice(),
                        work.as_bytes().as_slice(),
                        position as i64
                    ],
                )?;
            }
            for (member, preview) in suggestion.members {
                if let Some(old) = member.download_id {
                    let source: Option<(String, String)> = self
                        .conn
                        .query_row(
                            "SELECT source,source_id FROM downloads WHERE id=?1",
                            [old],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    if source.is_some_and(|(p, k)| p != member.source || k != member.source_id) {
                        return Err(Error::Integrity(
                            "提案内の作品の識別が矛盾しています".into(),
                        ));
                    }
                }
                let work = Library::relation_entity(
                    &tx,
                    &member.source,
                    EntityKind::Work,
                    &member.source_id,
                    row.time("created_at")?,
                )?;
                let (file, name) = match preview {
                    Some(preview) => {
                        Library::register_file(&tx, &preview.file)?;
                        self.mark_used_file(&tx, &preview.path, &preview.file.file.id.0)?;
                        (Some(preview.file.file.id.0.to_vec()), Some(preview.name))
                    }
                    None => (None, None),
                };
                tx.execute("INSERT INTO collection_suggestion_members VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![id.as_bytes().as_slice(),work.as_bytes().as_slice(),member.title,member.author_name,file,name,member.text_length,member.proposed_position,member.score,member.selected,serde_json::to_string(&member.evidence)?])?;
            }
            mark_row(&tx, "collection_suggestions", old, id)?;
        }
        mark_table(&tx, "collection_suggestions")?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Fingerprint, LibraryId, SuggestionDecision};
    use serde_json::json;
    use std::{fs, path::PathBuf, sync::atomic::AtomicBool};
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    impl Fixture {
        fn new(invalid: bool) -> Self {
            let root =
                std::env::temp_dir().join(format!("piep-suggestions-{}", LibraryId::now_v7()));
            let original = root.join("old/downloads/pixiv/one/v1/original.json");
            fs::create_dir_all(original.parent().unwrap()).unwrap();
            fs::write(
                &original,
                br#"{"detail":{"title":"current title"},"text":"body"}"#,
            )
            .unwrap();
            let preview = original.with_file_name("preview.png");
            fs::write(&preview, b"preview bytes with ICC and EXIF").unwrap();
            let old = rusqlite::Connection::open(root.join("old/piep.db")).unwrap();
            old.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE collection_suggestions(id TEXT,seed_json TEXT,proposed_name TEXT,collection_kind TEXT,members_json TEXT,score REAL,rule_version TEXT,state TEXT,created_at TEXT,updated_at TEXT,track TEXT,origin TEXT,evidence_summary TEXT,name_options_json TEXT);
INSERT INTO downloads VALUES(1,'pixiv','one','current title','author',0);").unwrap();
            old.execute(
                "INSERT INTO download_versions VALUES(10,1,1,?1,?1,'source')",
                [original.to_str().unwrap()],
            )
            .unwrap();
            for (index, decision) in ["pending", "accepted", "rejected"].into_iter().enumerate() {
                let members = json!([
                    {"source":"pixiv","sourceId":"one","downloadId":1,"title":"当時の題名","authorName":"当時の作者","coverPath":preview.to_str(),"textLength":42,"proposedPosition":9,"score":1.0,"selected":true,"evidence":[{"kind":"manual","label":"残す根拠","contribution":0.19850543076707122}]},
                    {"source":"pixiv","sourceId":"unsaved","downloadId":9999,"title":"未保存","authorName":"作者","coverPath":null,"textLength":15,"proposedPosition":if invalid && index==1 {9}else{2},"score":0.6038061332026103,"selected":false,"evidence":[]}
                ]);
                old.execute("INSERT INTO collection_suggestions VALUES(?1,'[1]','提案','ordered',?2,0.9,'rule-v1',?3,'2026-01-01 00:00:00','2026-01-02 00:00:00','sequence','seed','説明',?4)",params![index.to_string(),members.to_string(),decision,json!([{"source":"llm","name":"候補名","label":"名前の根拠","modelId":"model-v1"}]).to_string()]).unwrap();
            }
            Self(root)
        }
    }

    fn imported_id(target: &Library, key: &str) -> LibraryId {
        let bytes:Vec<u8> = target.conn.lock().unwrap().query_row("SELECT target_id FROM migration_records WHERE source_table='collection_suggestions' AND source_key=?1",[key],|r|r.get(0)).unwrap();
        LibraryId::from_slice(&bytes).unwrap()
    }

    #[test]
    fn suggestion_history_keeps_decisions_labels_unsaved_members_and_exact_previews_after_backup() {
        let fixture = Fixture::new(false);
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let target = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&target, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let mut saved = Vec::new();
        for (key, decision) in [
            ("0", SuggestionDecision::Pending),
            ("1", SuggestionDecision::Accepted),
            ("2", SuggestionDecision::Rejected),
        ] {
            let id = imported_id(&target, key);
            let suggestion = target.collection_suggestion(id).unwrap();
            assert_eq!(suggestion.decision, decision);
            assert_eq!(suggestion.seeds[0], suggestion.members[1].work);
            assert_eq!(suggestion.members[0].position, 2);
            assert!(!suggestion.members[0].selected);
            assert_eq!(suggestion.members[0].score, 0.6038061332026103);
            assert!(suggestion.members[1].evidence[0]
                .original_json()
                .contains("0.19850543076707122"));
            assert!(serde_json::to_string(&suggestion)
                .unwrap()
                .contains("0.19850543076707122"));
            assert_eq!(suggestion.members[1].title, "当時の題名");
            assert_eq!(suggestion.members[1].author, "当時の作者");
            assert_eq!(
                serde_json::from_str::<Value>(suggestion.name_options[0].original_json()).unwrap()
                    ["modelId"],
                "model-v1"
            );
            assert_eq!(
                suggestion.members[1].preview,
                Some(Fingerprint::of(b"preview bytes with ICC and EXIF"))
            );
            assert_eq!(
                serde_json::from_str::<Value>(suggestion.members[1].evidence[0].original_json())
                    .unwrap()["label"],
                "残す根拠"
            );
            saved.push(suggestion);
        }
        // 再送でローカルの新しい判断を古い状態へ戻さない。
        target
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE collection_suggestions SET decision='rejected',generation=2 WHERE id=?1",
                [saved[0].id.as_bytes().as_slice()],
            )
            .unwrap();
        assert!(source
            .import(&target, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        saved[0].decision = SuggestionDecision::Rejected;
        saved[0].generation = 2;
        assert_eq!(target.collection_suggestion(saved[0].id).unwrap(), saved[0]);
        assert_eq!(target.works(10).unwrap().len(), 1); // 未保存の候補を保存済みとは扱わない。
        let backup = fixture.0.join("backup");
        target.backup(&backup).unwrap();
        drop(target);
        drop(source);
        fs::rename(fixture.0.join("old"), fixture.0.join("old-unavailable")).unwrap();
        fs::rename(fixture.0.join("new"), fixture.0.join("new-unavailable")).unwrap();
        let copy = Library::open(&backup).unwrap();
        for suggestion in saved {
            assert_eq!(
                copy.collection_suggestion(suggestion.id).unwrap(),
                suggestion
            );
        }
        copy.verify().unwrap();
    }

    #[test]
    fn an_invalid_suggestion_rolls_back_earlier_suggestions_and_blocks_activation() {
        let fixture = Fixture::new(true);
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let target = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&target, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(!report.unresolved.is_empty());
        let conn = target.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM collection_suggestions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM collection_suggestion_members",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM works", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(conn);
        assert!(!target.activation_problems().unwrap().is_empty());
        assert!(source
            .import(&target, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .iter()
            .any(|p| p.contains("コレクション提案")));
        target.verify().unwrap();
    }
}
