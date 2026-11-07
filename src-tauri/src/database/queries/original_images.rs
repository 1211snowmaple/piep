//! 縮小版で保存された pixiv の画像を、原寸に置き換えるための問い合わせ。
//!
//! 古い保存の表紙は `c/240x480_80` の縮小版、参照イラストは 1200px の版で
//! 入っている。原寸に替えても作品の中身は変わらないので、**版は増やさない**
//! （R-18 の代替表紙を直す `replace_placeholder_cover` と同じ考え方）。
//! 保存した JSON（取得元の写し）も書き換えない。替えるのは資産の行と、
//! 表紙なら作品の `cover_path` だけである。

use super::{asset_belongs_to_version_path, Database};
use crate::downloader::pixiv_originals::is_reduced_pixiv_image;
use rusqlite::params;
use std::path::Path;

/// 原寸に取り直す対象の画像一枚。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedPixivImage {
    pub asset_id: i64,
    pub download_id: i64,
    pub title: String,
    /// `cover` か `illustration`。
    pub asset_type: String,
    pub local_path: String,
    pub original_url: String,
}

/// 差し替えを記録した結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReducedImageReplacement {
    /// 行を書き換えた。`false` なら、その間に作品が保存し直されたなどで行が
    /// 変わっていた（新しいファイルは使われていない）。
    pub replaced: bool,
    /// 古いファイルを指す行がもう無い。消してよい。
    pub old_file_unreferenced: bool,
}

impl Database {
    /// 今の版に入っている、縮小版の pixiv の表紙・イラスト。
    ///
    /// 過去の版の画像は数えない - 読むのも書き出すのも今の版で、過去の版は
    /// 取得したときの姿のまま残しておく。
    pub fn reduced_pixiv_images(&self) -> Result<Vec<ReducedPixivImage>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT a.id, a.download_id, d.title, a.asset_type, a.local_path, a.original_url,
                        COALESCE(d.original_json_path, d.json_path)
                 FROM assets a JOIN downloads d ON d.id = a.download_id
                 WHERE d.source = 'pixiv'
                   AND a.asset_type IN ('cover', 'illustration')
                   AND a.original_url IS NOT NULL
                   AND (instr(a.original_url, 'novel-cover-master/') > 0
                        OR instr(a.original_url, '/img-master/') > 0)
                 ORDER BY d.id, a.id",
            )
            .map_err(|e| format!("Failed to prepare reduced images: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    ReducedPixivImage {
                        asset_id: row.get(0)?,
                        download_id: row.get(1)?,
                        title: row.get(2)?,
                        asset_type: row.get(3)?,
                        local_path: row.get(4)?,
                        original_url: row.get(5)?,
                    },
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(|e| format!("Failed to list reduced images: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read reduced images: {e}"))?;
        Ok(rows
            .into_iter()
            .filter(|(image, json_path)| {
                is_reduced_pixiv_image(&image.original_url)
                    && asset_belongs_to_version_path(
                        Path::new(&image.local_path),
                        Path::new(json_path),
                    )
            })
            .map(|(image, _)| image)
            .collect())
    }

    /// 縮小版の画像を原寸のファイルに差し替えたことを記録する。
    ///
    /// 行が `old_path` を指したままのときだけ書き換える。作品の大きさは、
    /// 古いファイルとの差だけ増減させる。
    #[allow(clippy::too_many_arguments)]
    pub fn replace_reduced_pixiv_image(
        &self,
        asset_id: i64,
        download_id: i64,
        old_path: &str,
        new_path: &str,
        url: &str,
        mime_type: Option<&str>,
        bytes: i64,
    ) -> Result<ReducedImageReplacement, String> {
        let filename = Path::new(new_path)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("差し替える画像の名前がありません")?
            .to_string();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let old_bytes: Option<i64> = tx
            .query_row(
                "SELECT file_size_bytes FROM assets WHERE id=?1 AND download_id=?2 AND local_path=?3",
                params![asset_id, download_id, old_path],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            })
            .map_err(|e| format!("Failed to read the reduced image: {e}"))?;
        let Some(old_bytes) = old_bytes else {
            return Ok(ReducedImageReplacement {
                replaced: false,
                old_file_unreferenced: false,
            });
        };
        tx.execute(
            "UPDATE assets SET filename=?3, local_path=?4, original_url=?5, mime_type=?6, file_size_bytes=?7
             WHERE id=?1 AND download_id=?2",
            params![asset_id, download_id, filename, new_path, url, mime_type, bytes],
        )
        .map_err(|e| format!("Failed to record the original image: {e}"))?;
        tx.execute(
            "UPDATE downloads SET cover_path=?3 WHERE id=?1 AND cover_path=?2",
            params![download_id, old_path, new_path],
        )
        .map_err(|e| format!("Failed to point the cover at the original: {e}"))?;
        tx.execute(
            "UPDATE downloads SET file_size_bytes=MAX(0, file_size_bytes + ?2 - ?3) WHERE id=?1",
            params![download_id, bytes, old_bytes],
        )
        .map_err(|e| format!("Failed to count the original image: {e}"))?;
        let still_referenced: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM assets WHERE local_path=?1)
                     OR EXISTS(SELECT 1 FROM downloads WHERE cover_path=?1)",
                params![old_path],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check the old image: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit the original image: {e}"))?;
        Ok(ReducedImageReplacement {
            replaced: true,
            old_file_unreferenced: old_path != new_path && !still_referenced,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::database::{Database, NewAsset, NewDownload, NewVersion};

    struct TempLibrary(std::path::PathBuf);

    impl Drop for TempLibrary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn library() -> (TempLibrary, Database) {
        let root =
            std::env::temp_dir().join(format!("piep_original_images_{}", rand::random::<u64>()));
        let storage = root.join("downloads");
        std::fs::create_dir_all(&storage).unwrap();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        (TempLibrary(root), db)
    }

    #[test]
    fn only_reduced_images_of_the_current_version_are_listed_and_replaced_in_place() {
        let (dir, db) = library();
        let root = dir.0.join("downloads/pixiv/101");
        let (v1, v2) = (root.join("v1"), root.join("v2"));
        let path = |version: &std::path::Path, folder: &str, name: &str| {
            version
                .join("data_assets")
                .join(folder)
                .join(name)
                .to_string_lossy()
                .to_string()
        };
        let cover_240 = "https://i.pximg.net/c/240x480_80/novel-cover-master/img/2024/01/01/00/00/00/ci101_abc_master1200.jpg";
        let asset = |asset_type: &str, local_path: String, url: &str, bytes: i64| NewAsset {
            download_id: 0,
            asset_type: asset_type.into(),
            filename: std::path::Path::new(&local_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into(),
            local_path,
            original_url: Some(url.into()),
            mime_type: Some("image/jpeg".into()),
            file_size_bytes: bytes,
        };
        let current_cover = path(&v2, "cover", "ci101_abc_master1200.jpg");
        let assets =
            vec![
            asset("cover", path(&v1, "cover", "ci101_abc_master1200.jpg"), cover_240, 20),
            asset("cover", current_cover.clone(), cover_240, 20),
            asset(
                "illustration",
                path(&v2, "illustrations", "70731269-1_p0.jpg"),
                "https://i.pximg.net/img-master/img/2018/09/17/00/00/11/70731269_p0_master1200.jpg",
                300,
            ),
            asset(
                "illustration",
                path(&v2, "illustrations", "inline_5.png"),
                "https://i.pximg.net/novel-cover-original/img/2024/01/01/00/00/00/tei5_x.png",
                900,
            ),
        ];
        let json = v2.join("original.json").to_string_lossy().to_string();
        let download = NewDownload {
            source: "pixiv".into(),
            source_id: "101".into(),
            title: "作品".into(),
            author_name: "作者".into(),
            author_id: "1".into(),
            content_type: "novel".into(),
            tags: vec![],
            excerpt: None,
            cover_path: Some(current_cover.clone()),
            json_path: json.clone(),
            original_json_path: Some(json.clone()),
            asset_count: 4,
            file_size_bytes: 2000,
            downloaded_at: "2026-11-07T00:00:00Z".into(),
            source_created_at: None,
            content_hash: Some("h".into()),
            text_length: 1,
            source_updated_at: None,
            watch_updates: false,
            current_version: 2,
            favorite: false,
        };
        let version = NewVersion {
            download_id: 0,
            version: 2,
            content_hash: Some("h".into()),
            text_length: 1,
            json_path: json.clone(),
            original_json_path: Some(json),
            asset_count: 4,
            file_size_bytes: 2000,
            created_at: "2026-11-07T00:00:00Z".into(),
            change_summary: None,
        };
        let id = db
            .commit_reimported_download(&download, &assets, &[version])
            .unwrap();

        let reduced = db.reduced_pixiv_images().unwrap();
        assert_eq!(
            reduced
                .iter()
                .map(|image| image.local_path.as_str())
                .collect::<Vec<_>>(),
            vec![
                current_cover.as_str(),
                path(&v2, "illustrations", "70731269-1_p0.jpg").as_str()
            ],
            "過去の版と原寸の画像は対象外"
        );

        let cover = &reduced[0];
        let new_cover = path(&v2, "cover", "ci101_abc.png");
        let original =
            "https://i.pximg.net/novel-cover-original/img/2024/01/01/00/00/00/ci101_abc.png";
        let outcome = db
            .replace_reduced_pixiv_image(
                cover.asset_id,
                id,
                &cover.local_path,
                &new_cover,
                original,
                Some("image/png"),
                500,
            )
            .unwrap();
        assert!(outcome.replaced);
        assert!(outcome.old_file_unreferenced);
        let saved = db.get_download(id).unwrap();
        assert_eq!(saved.cover_path.as_deref(), Some(new_cover.as_str()));
        assert_eq!(saved.file_size_bytes, 2000 + 500 - 20);
        assert_eq!(saved.current_version, 2, "版は増やさない");
        let row = db
            .get_assets(id)
            .unwrap()
            .into_iter()
            .find(|asset| asset.id == cover.asset_id)
            .unwrap();
        assert_eq!(row.filename, "ci101_abc.png");
        assert_eq!(row.original_url.as_deref(), Some(original));
        assert_eq!(row.mime_type.as_deref(), Some("image/png"));
        assert_eq!(db.reduced_pixiv_images().unwrap().len(), 1);

        // 同じ名前のまま中身だけ替えたときは、古いファイルを消させない。
        let illust = &db.reduced_pixiv_images().unwrap()[0];
        let same = db
            .replace_reduced_pixiv_image(
                illust.asset_id,
                id,
                &illust.local_path,
                &illust.local_path,
                "https://i.pximg.net/img-original/img/2018/09/17/00/00/11/70731269_p0.jpg",
                Some("image/jpeg"),
                700,
            )
            .unwrap();
        assert!(same.replaced);
        assert!(!same.old_file_unreferenced);
        assert!(db.reduced_pixiv_images().unwrap().is_empty());

        // 行が先に変わっていたら何もしない。
        let stale = db
            .replace_reduced_pixiv_image(
                cover.asset_id,
                id,
                &cover.local_path,
                &new_cover,
                original,
                None,
                1,
            )
            .unwrap();
        assert!(!stale.replaced);
        assert_eq!(
            db.get_download(id).unwrap().file_size_bytes,
            2000 + 500 - 20 + 700 - 300
        );
    }
}
