//! 問い合わせの中の絞り込みの書き方（`tag:` `author:` `series:` `source:` `id:`、作品の URL）。
//!
//! 画面は作品のタグから `tag:…`、検索の候補から `author:"…"` `series:"…"` を作り、利用者も
//! 手で打つ。本文の検索に渡す前に一覧の絞り込みへ移す。
//!
//! 決まり:
//! - `id:` と作品の URL は、取得元の ID がそのまま一致する作品に絞る。ID の数字を検索語として
//!   本文の索引に渡すと、本文に同じ数字が出てくる作品も当たり、ほかの語と一緒に書くと一つも
//!   当たらない。複数書けば、そのどれか（URL を並べて貼った作品を一度に出す）。
//! - `series:` の題は大文字小文字を区別しない（作者やシリーズの指名と同じ）。
//! - 一覧の絞り込みとぶつかる取得元（`source:` と画面の取得元が違う）は、書いた側を勝たせず、
//!   両方を満たす作品（つまりなし）にする。

use crate::library::{Error, Library, LibraryId, Result, WorkFilter};

/// 問い合わせから読み取った絞り込みと、本文の検索に残る語。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryParts {
    /// 本文の検索に渡す語（絞り込みの書き方を除いたもの）。
    pub text: String,
    pub tags: Vec<String>,
    pub excluded_tags: Vec<String>,
    pub authors: Vec<String>,
    pub excluded_authors: Vec<String>,
    /// `series:`。どれも満たす作品に絞る。
    pub series: Vec<SeriesRef>,
    /// `source:` と URL の取得元。
    pub providers: Vec<String>,
    /// `id:` と URL。どれか一つに当たる作品に絞る。
    pub works: Vec<WorkRef>,
}

/// `series:` の値。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeriesRef {
    /// `series:pixiv:123`。取得元のシリーズの ID。
    Key { provider: String, key: String },
    /// `series:123` や `series:"題"`。取得元のシリーズの ID か題のどちらか。
    KeyOrTitle(String),
}

/// `id:` と URL の値。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkRef {
    /// URL から分かった取得元。`id:` だけなら決まらない。
    pub provider: Option<&'static str>,
    pub key: String,
}

impl QueryParts {
    /// 絞り込みを一つも書いていないか。
    pub fn is_plain(&self) -> bool {
        self.tags.is_empty()
            && self.excluded_tags.is_empty()
            && self.authors.is_empty()
            && self.excluded_authors.is_empty()
            && self.series.is_empty()
            && self.providers.is_empty()
            && self.works.is_empty()
    }
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|item| item == value) {
        list.push(value.to_owned());
    }
}

/// 空白で区切る。引用符の中の空白では区切らない。
fn split_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut term = String::new();
    let mut quoted = false;
    for c in query.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && c.is_whitespace() {
            if !term.trim().is_empty() {
                terms.push(std::mem::take(&mut term));
            }
            term.clear();
            continue;
        }
        term.push(c);
    }
    if !term.trim().is_empty() {
        terms.push(term);
    }
    terms
}

/// 作品の URL から取得元と ID を取る。
pub fn work_from_url(value: &str) -> Option<(&'static str, String)> {
    let url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "https" | "http") {
        return None;
    }
    let host = url.host_str()?;
    let segments: Vec<&str> = url.path_segments()?.collect();
    let (provider, id) = if host == "pixiv.net" || host == "www.pixiv.net" {
        let id = if url.path().ends_with("/novel/show.php") {
            url.query_pairs()
                .find(|(key, _)| key == "id")?
                .1
                .into_owned()
        } else {
            let at = segments
                .iter()
                .position(|segment| matches!(*segment, "novels" | "artworks"))?;
            segments.get(at + 1)?.to_string()
        };
        ("pixiv", id)
    } else if host == "fanbox.cc" || host.ends_with(".fanbox.cc") {
        let at = segments.iter().position(|segment| *segment == "posts")?;
        ("fanbox", segments.get(at + 1)?.to_string())
    } else {
        return None;
    };
    (!id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_digit()))
        .then_some((provider, id))
}

/// 問い合わせを絞り込みと本文の語に分ける。知らない項目名（`foo:bar`）と、除外を書けない
/// 項目の除外（`-series:` `-source:` `-id:`）は、語のまま本文の検索に残す。
pub fn parse_query(query: &str) -> QueryParts {
    let mut parts = QueryParts::default();
    let mut text = Vec::new();
    for raw in split_terms(query) {
        let excluded = raw.starts_with('-');
        let term = raw.trim_start_matches('-');
        // URL も `:` を含むので、項目名より先に見る。
        if !excluded {
            if let Some((provider, key)) = work_from_url(term) {
                push_unique(&mut parts.providers, provider);
                parts.works.push(WorkRef {
                    provider: Some(provider),
                    key,
                });
                continue;
            }
        }
        let Some((field, value)) = term.split_once(':') else {
            text.push(raw);
            continue;
        };
        let field = field.to_ascii_lowercase();
        let value = value.trim_matches('"').trim();
        if value.is_empty() {
            continue;
        }
        match (field.as_str(), excluded) {
            ("tag" | "tags", false) => push_unique(&mut parts.tags, value),
            ("tag" | "tags", true) => push_unique(&mut parts.excluded_tags, value),
            ("author" | "creator", false) => push_unique(&mut parts.authors, value),
            ("author" | "creator", true) => push_unique(&mut parts.excluded_authors, value),
            ("series" | "series_key" | "series_id", false) => {
                parts.series.push(match value.split_once(':') {
                    Some((provider, key))
                        if !provider.trim().is_empty() && !key.trim().is_empty() =>
                    {
                        SeriesRef::Key {
                            provider: provider.trim().to_ascii_lowercase(),
                            key: key.trim().to_owned(),
                        }
                    }
                    _ => SeriesRef::KeyOrTitle(value.to_owned()),
                })
            }
            ("title" | "series_title", _) => text.push(if excluded {
                format!("-{value}")
            } else {
                value.to_owned()
            }),
            ("source", false) => push_unique(&mut parts.providers, &value.to_ascii_lowercase()),
            ("id" | "source_id" | "sourceid" | "url", false) => {
                parts.works.push(match work_from_url(value) {
                    Some((provider, key)) => {
                        push_unique(&mut parts.providers, provider);
                        WorkRef {
                            provider: Some(provider),
                            key,
                        }
                    }
                    None => WorkRef {
                        provider: None,
                        key: value.to_owned(),
                    },
                })
            }
            _ => text.push(raw),
        }
    }
    parts.text = text.join(" ");
    parts
}

fn ids(rows: Vec<Vec<u8>>) -> Result<Vec<LibraryId>> {
    rows.iter()
        .map(|bytes| {
            LibraryId::from_slice(bytes).map_err(|_| Error::Integrity("ID が不正です".into()))
        })
        .collect()
}

/// `works` を `only` の作品だけに狭める（まだ狭めていなければ `only` そのもの）。
fn narrow(works: &mut Option<Vec<LibraryId>>, only: Vec<LibraryId>) {
    *works = Some(match works.take() {
        None => only,
        Some(mut current) => {
            current.retain(|work| only.contains(work));
            current
        }
    });
}

impl Library {
    /// 読み取った絞り込みを一覧の絞り込みへ足す。どれも「かつ」でつなぐ。
    pub(crate) fn apply_query_parts(
        &self,
        parts: &QueryParts,
        filter: &mut WorkFilter,
    ) -> Result<()> {
        for tag in &parts.tags {
            push_unique(&mut filter.tags, tag);
        }
        for tag in &parts.excluded_tags {
            push_unique(&mut filter.excluded_tags, tag);
        }
        for author in &parts.authors {
            push_unique(&mut filter.authors, author);
        }
        for author in &parts.excluded_authors {
            push_unique(&mut filter.excluded_authors, author);
        }
        for provider in &parts.providers {
            match &filter.provider {
                Some(current) if current != provider => {
                    filter.works = Some(Vec::new());
                }
                _ => filter.provider = Some(provider.clone()),
            }
        }
        let conn = self.reader()?;
        for series in &parts.series {
            let found: Vec<Vec<u8>> = match series {
                SeriesRef::Key { provider, key } => conn
                    .prepare_cached(
                        "SELECT entity_id FROM external_identities
                         WHERE provider=?1 AND kind='series' AND external_key=?2",
                    )?
                    .query_map([provider, key], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?,
                SeriesRef::KeyOrTitle(value) => conn
                    .prepare_cached(
                        "SELECT entity_id FROM external_identities WHERE kind='series' AND external_key=?1
                         UNION SELECT DISTINCT series_id FROM snapshot_series WHERE title=?1 COLLATE NOCASE",
                    )?
                    .query_map([value], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?,
            };
            let found = ids(found)?;
            match found.as_slice() {
                [one] if filter.series.is_none() => filter.series = Some(*one),
                [one] if filter.series == Some(*one) => {}
                _ => {
                    // シリーズが一つに決まらない（同じ題が複数・見つからない・画面のシリーズと
                    // 別）ときは、どれかのシリーズに入っている作品に絞る。
                    let works = self.works_in_any_series(&found)?;
                    narrow(&mut filter.works, works);
                }
            }
        }
        if !parts.works.is_empty() {
            let mut found = Vec::new();
            let mut statement = conn.prepare_cached(
                "SELECT entity_id FROM external_identities
                 WHERE kind='work' AND external_key=?1 AND (?2 IS NULL OR provider=?2)",
            )?;
            for work in &parts.works {
                let rows: Vec<Vec<u8>> = statement
                    .query_map(rusqlite::params![work.key, work.provider], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                found.extend(ids(rows)?);
            }
            found.sort();
            found.dedup();
            narrow(&mut filter.works, found);
        }
        Ok(())
    }

    /// いまの版がこのシリーズのどれかに入っている作品。
    fn works_in_any_series(&self, series: &[LibraryId]) -> Result<Vec<LibraryId>> {
        if series.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.reader()?;
        let keys: Vec<String> = series.iter().map(|id| id.simple().to_string()).collect();
        let rows: Vec<Vec<u8>> = conn
            .prepare_cached(
                "SELECT DISTINCT p.work_id FROM snapshot_series ss
                 JOIN work_current_projection p ON p.snapshot_id=ss.snapshot_id
                 WHERE ss.series_id IN (SELECT unhex(value) FROM json_each(?1))",
            )?
            .query_map([serde_json::to_string(&keys)?], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        ids(rows)
    }
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
