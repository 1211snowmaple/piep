//! 本文の検索の文書。旧棚の全文索引と同じ材料を、新しい棚の保存状態から作る。
//!
//! 索引は派生物で、関連度の重みと字句解析は旧棚の実装をそのまま使う
//! （`crate::database::tantivy_index`）。ここは「何を索引に入れるか」だけを決める。
//! 旧棚と食い違うと、同じ問い合わせで出る作品が棚の切り替えで変わるので、全作品の
//! 文書を旧棚の文書と突き合わせて確かめる（`piep-library compare-search-documents`）。

use crate::database::models::AssetEntry;
use crate::library::{Error, Fingerprint, Library, LibraryId, Result, SourceBody};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use std::path::PathBuf;

/// 一作品の検索の文書。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchDocument {
    pub work: LibraryId,
    pub provider: String,
    pub external_key: String,
    /// 取得元の作者の ID（pixiv の利用者番号、FANBOX のクリエイター ID）。
    pub author_key: String,
    pub title: String,
    pub author_name: String,
    /// 保存版のタグと利用者のタグ。名前の順。
    pub tags: Vec<String>,
    pub series_titles: Vec<String>,
    pub excerpt: String,
    /// 有効な編集があればその本文、なければ保存版の本文（FANBOX は添付から取り出した本文を含む）。
    pub body: String,
    /// 保存版にある画像・添付の種類（旧棚の資産の種類の名前: cover・illustration・file）。
    pub asset_kinds: Vec<&'static str>,
    pub text_length: i64,
    pub saved_at: i64,
    pub published_at: Option<i64>,
}

/// 取得元の作品の URL。旧棚の索引と同じ形。
pub fn source_url(provider: &str, external_key: &str, author_key: &str) -> String {
    match provider {
        "pixiv" => format!("https://www.pixiv.net/novel/show.php?id={external_key}"),
        "fanbox" if !author_key.is_empty() => {
            format!("https://{author_key}.fanbox.cc/posts/{external_key}")
        }
        "fanbox" => format!("https://www.fanbox.cc/posts/{external_key}"),
        _ => external_key.to_owned(),
    }
}

impl Library {
    /// 作品の検索の文書。棚にない作品なら `None`。
    pub fn search_document(&self, work: LibraryId) -> Result<Option<SearchDocument>> {
        let conn = self.reader()?;
        self.search_document_from_conn(&conn, work)
    }

    pub(super) fn search_document_from_conn(
        &self,
        conn: &Connection,
        work: LibraryId,
    ) -> Result<Option<SearchDocument>> {
        let key = work.as_bytes().to_vec();
        let row = conn
            .query_row(
                "SELECT p.snapshot_id,p.edit_id,p.provider,p.external_key,p.title,p.author_name,
                        p.text_length,p.saved_at,p.published_at,
                        CAST(coalesce(json_extract(s.metadata_json,'$.author_key'),'') AS TEXT),
                        CAST(coalesce(json_extract(s.metadata_json,'$.caption'),json_extract(s.metadata_json,'$.excerpt'),'') AS TEXT)
                 FROM work_current_projection p JOIN revision_snapshots s ON s.id=p.snapshot_id
                 WHERE p.work_id=?1",
                [&key],
                |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, Option<Vec<u8>>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, Option<i64>>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, String>(10)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            snapshot,
            edit,
            provider,
            external_key,
            title,
            author_name,
            text_length,
            saved_at,
            published_at,
            author_key,
            caption,
        )) = row
        else {
            return Ok(None);
        };
        // 保存版の説明をそのまま使う。旧棚の作品の行の説明は、取得の経路によって前後の
        // 空白が落ちていることがある（版の原本には残っている）。前後の空白は語にならない
        // ので、索引で引ける語は変わらない。
        let head = Head {
            provider,
            external_key,
            author_key,
            title,
            author_name,
            excerpt: caption,
            text_length,
            saved_at,
            published_at,
        };
        self.complete_document(conn, work, &snapshot, edit.as_deref(), head)
            .map(Some)
    }

    /// 過去の改稿の文書。題・作者・説明・タグ・シリーズ・本文はすべてその改稿の保存版の
    /// もの（編集は今の版にしか付かないので使わない）。利用者のタグは今のものを入れる。
    pub(super) fn past_search_document(
        &self,
        conn: &Connection,
        work: LibraryId,
        snapshot: &[u8],
    ) -> Result<Option<SearchDocument>> {
        let row = conn
            .query_row(
                &format!(
                    "SELECT p.provider,p.external_key,
                        trim(CAST(coalesce(json_extract(s.metadata_json,'$.title'),'') AS TEXT),{SPACES}),
                        CAST(coalesce(json_extract(s.metadata_json,'$.author_name'),'') AS TEXT),
                        coalesce((SELECT t.text_length FROM body_text_stats t WHERE t.file_id=s.body_file_id),0),
                        r.discovered_at,
                        CASE json_type(s.metadata_json,'$.create_date') WHEN 'integer' THEN json_extract(s.metadata_json,'$.create_date') END,
                        CAST(coalesce(json_extract(s.metadata_json,'$.author_key'),'') AS TEXT),
                        CAST(coalesce(json_extract(s.metadata_json,'$.caption'),json_extract(s.metadata_json,'$.excerpt'),'') AS TEXT)
                     FROM revision_snapshots s JOIN source_revisions r ON r.id=s.revision_id
                     JOIN work_current_projection p ON p.work_id=s.entity_id
                     WHERE s.id=?1 AND s.entity_id=?2"
                ),
                params![snapshot, work.as_bytes().as_slice()],
                |r| {
                    Ok(Head {
                        provider: r.get(0)?,
                        external_key: r.get(1)?,
                        title: r.get(2)?,
                        author_name: r.get(3)?,
                        text_length: r.get(4)?,
                        saved_at: r.get(5)?,
                        published_at: r.get(6)?,
                        author_key: r.get(7)?,
                        excerpt: r.get(8)?,
                    })
                },
            )
            .optional()?;
        let Some(head) = row else {
            return Ok(None);
        };
        self.complete_document(conn, work, snapshot, None, head)
            .map(Some)
    }

    /// 保存版から、タグ・シリーズ・資産の種類・本文を足して文書にする。
    fn complete_document(
        &self,
        conn: &Connection,
        work: LibraryId,
        snapshot: &[u8],
        edit: Option<&[u8]>,
        head: Head,
    ) -> Result<SearchDocument> {
        let key = work.as_bytes().to_vec();
        let snapshot_id = id(snapshot)?;
        let mut tags: Vec<String> = conn
            .prepare_cached(
                "SELECT t.name FROM snapshot_tags st JOIN tags t ON t.id=st.tag_id WHERE st.snapshot_id=?1
                 UNION SELECT u.name FROM work_user_tags wt JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id
                 WHERE wt.work_id=?2 AND wt.user_id=?3",
            )?
            .query_map(params![snapshot, key, self.id.as_bytes().as_slice()], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        tags.sort();
        let series_titles: Vec<String> = conn
            .prepare_cached(
                "SELECT title FROM snapshot_series WHERE snapshot_id=?1 ORDER BY position,title",
            )?
            .query_map([&snapshot], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut asset_kinds = Vec::new();
        let roles: Vec<(String, String)> = conn
            .prepare_cached(
                "SELECT 'media',role FROM snapshot_media WHERE snapshot_id=?1
                 UNION SELECT 'file',role FROM snapshot_files WHERE snapshot_id=?1 AND role<>'raw'",
            )?
            .query_map([&snapshot], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (table, role) in roles {
            let kind = match (table.as_str(), role.as_str()) {
                ("media", "cover") => "cover",
                ("media", _) => "illustration",
                _ => "file",
            };
            if !asset_kinds.contains(&kind) {
                asset_kinds.push(kind);
            }
        }
        asset_kinds.sort_unstable();
        let body = match edit {
            Some(edit) => self.edit_search_body(conn, edit)?,
            None => self.snapshot_search_body(conn, work, snapshot_id, &head.provider)?,
        };
        Ok(SearchDocument {
            work,
            provider: head.provider,
            external_key: head.external_key,
            author_key: head.author_key,
            title: head.title,
            author_name: head.author_name,
            tags,
            series_titles,
            excerpt: head.excerpt,
            body,
            asset_kinds,
            text_length: head.text_length,
            saved_at: head.saved_at,
            published_at: head.published_at,
        })
    }

    /// 有効な編集の本文。旧棚と同じく、画像・区切り・改ページ以外の段落の文を空行でつなぐ。
    fn edit_search_body(&self, conn: &Connection, edit: &[u8]) -> Result<String> {
        let file: Vec<u8> = conn.query_row(
            "SELECT content_file_id FROM edit_revisions WHERE id=?1",
            [edit],
            |r| r.get(0),
        )?;
        let content = self.edit_content(conn, &file)?;
        Ok(content
            .blocks
            .iter()
            .filter(|block| !matches!(block.kind.as_str(), "image" | "separator" | "page_break"))
            .filter_map(|block| block.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n\n"))
    }

    /// 保存版の本文。FANBOX は、旧棚と同じく添付の PDF・テキストから取り出した本文を
    /// 添付の位置へ差し込んでから、段落と見出しの文を取り出す。
    fn snapshot_search_body(
        &self,
        conn: &Connection,
        work: LibraryId,
        snapshot: LibraryId,
        provider: &str,
    ) -> Result<String> {
        let Some(body) = self.source_body_from_conn(conn, work, snapshot)? else {
            return Ok(String::new());
        };
        let SourceBody::FanboxBody(body) = body else {
            return Ok(body.text_content());
        };
        let mut post = json!({ "body": body });
        self.insert_attachment_bodies(conn, snapshot, provider, &mut post)?;
        Ok(crate::database::search::extract_search_body(
            &post, "fanbox",
        ))
    }

    /// FANBOX の保存版の投稿（`{"body": …}`）へ、添付の PDF・テキストから取り出した本文を、
    /// 旧棚と同じく添付の位置へ差し込む。
    ///
    /// 検索の文書と読む画面の本文の両方がここを通す。別々に差し込むと「読めるのに検索に
    /// 出ない」作品ができる（旧棚の `content_from_json` と同じ理由）。
    pub(crate) fn insert_attachment_bodies(
        &self,
        conn: &Connection,
        snapshot: LibraryId,
        provider: &str,
        post: &mut serde_json::Value,
    ) -> Result<()> {
        let texts = self.attachment_text_files(conn, snapshot)?;
        if texts.files.is_empty() {
            return Ok(());
        }
        crate::database::attachment::apply_attachment_text(post, provider, &texts.files);
        // 差し込んだ印の `localPath`（「原本を見る」）は一時に書き出したファイルを指していて、
        // ここを出ると消える。添付のファイルの識別子に置き換え、原本はそれで開く
        // （`shelf_render_pdf_page`）。知らない場所を指す印は外す（押しても開けない）。
        if let Some(blocks) = post
            .pointer_mut("/body/blocks")
            .and_then(serde_json::Value::as_array_mut)
        {
            for block in blocks {
                if block.get("type").and_then(serde_json::Value::as_str)
                    != Some(crate::database::attachment::NOTICE_BLOCK_TYPE)
                {
                    continue;
                }
                let Some(block) = block.as_object_mut() else {
                    continue;
                };
                let file = block
                    .get("localPath")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|path| {
                        texts
                            .files
                            .iter()
                            .position(|entry| entry.local_path == path)
                    })
                    .map(|index| texts.fingerprints[index].to_string());
                match file {
                    Some(file) => {
                        block.insert("localPath".into(), file.into());
                    }
                    None => {
                        block.remove("localPath");
                    }
                }
            }
        }
        Ok(())
    }

    /// 本文として取り込める添付（PDF・テキスト）を一時のフォルダーへ書き出す。旧棚の
    /// 取り出しはファイルの場所を受け取るので、実体を一時的に戻して渡す。
    fn attachment_text_files(&self, conn: &Connection, snapshot: LibraryId) -> Result<TextFiles> {
        let rows: Vec<(Vec<u8>, String, Option<String>)> = conn
            .prepare_cached(
                "SELECT file_id,original_name,mime_type FROM snapshot_files
                 WHERE snapshot_id=?1 AND role='attachment'
                   AND (lower(original_name) LIKE '%.pdf' OR lower(original_name) LIKE '%.txt')
                 ORDER BY slot",
            )?
            .query_map([snapshot.as_bytes().as_slice()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut texts = TextFiles {
            dir: None,
            files: Vec::new(),
            fingerprints: Vec::new(),
        };
        if rows.is_empty() {
            return Ok(texts);
        }
        let dir =
            std::env::temp_dir().join(format!("piep-search-{}", LibraryId::now_v7().simple()));
        std::fs::create_dir_all(&dir)?;
        texts.dir = Some(dir.clone());
        for (index, (file, name, mime)) in rows.into_iter().enumerate() {
            let fingerprint = Fingerprint(
                file.try_into()
                    .map_err(|_| Error::Integrity("添付のハッシュが不正です".into()))?,
            );
            let content = Self::file_from_conn(conn, fingerprint)?;
            // 旧棚は添付を「名前が一致する」か「場所に取得元のファイル ID を含む」で探す。
            // 保存名は `名前_ID.拡張子` なので、保存名のまま置かないと見つからない。
            // フォルダーを分けて名前の衝突を避け、危ない文字は保存時と同じ規則で均す。
            let place = dir.join(index.to_string());
            std::fs::create_dir_all(&place)?;
            let path = place.join(crate::downloader::asset_downloader::sanitize_filename(
                &name,
            ));
            let mut output = std::fs::File::create(&path)?;
            self.objects.export(&content, &mut output)?;
            texts.files.push(AssetEntry {
                id: index as i64,
                download_id: 0,
                asset_type: "file".into(),
                filename: name,
                local_path: path.to_string_lossy().into_owned(),
                original_url: None,
                mime_type: mime,
                file_size_bytes: content.length as i64,
            });
            texts.fingerprints.push(fingerprint);
        }
        Ok(texts)
    }
}

/// 一時に書き出した添付。手放すとフォルダーごと消す。
struct TextFiles {
    dir: Option<PathBuf>,
    files: Vec<AssetEntry>,
    /// `files` と同じ並びの、棚のファイルの識別子。
    fingerprints: Vec<Fingerprint>,
}

impl Drop for TextFiles {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// 保存版ごとに違う、文書の頭の部分。
struct Head {
    provider: String,
    external_key: String,
    author_key: String,
    title: String,
    author_name: String,
    excerpt: String,
    text_length: i64,
    saved_at: i64,
    published_at: Option<i64>,
}

/// 一覧の題と同じく、前後から落とす空白（Unicode の空白すべて）。
const SPACES: &str = "char(9,10,11,12,13,32,133,160,5760,8192,8193,8194,8195,8196,8197,8198,8199,8200,8201,8202,8232,8233,8239,8287,12288)";

fn id(bytes: &[u8]) -> Result<LibraryId> {
    LibraryId::from_slice(bytes).map_err(|_| Error::Integrity("ID が不正です".into()))
}
