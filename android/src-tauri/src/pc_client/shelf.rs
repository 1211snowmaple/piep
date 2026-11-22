//! 棚の接続情報と読書用の応答を、登録した端末の認証で受け取る。
use super::{
    ClientError, PROBE_TIMEOUT, PcClientState, PcSearchCriteria, PcWorkScope, REQUEST_TIMEOUT,
    SavedConnection, bounded_response_bytes, check_response,
};
use piep_library_protocol::{
    Id, LibraryIdentity, MAX_IDENTITY_BYTES, MAX_MANIFEST_BYTES, ReaderBundle, ReaderWorkPage,
    ResourceReference, ResourceScope, WorkReadingView,
};
use piep_mobile_api::{
    MobileCollection, MobileEntityDetail, MobileEntityPage, MobileEntitySeriesPage,
    MobileSearchSuggestions, MobileShelfCollectionDetail, MobileShelfFavorite,
    MobileShelfFavoriteUpdate, MobileShelfPage, MobileShelfWork, MobileShelfWorkDetail,
};
use piep_storage::Fingerprint;
use reqwest::{Response, StatusCode, header::HeaderMap};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::State;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShelfScope {
    library_id: Id,
    subject_id: Id,
    sync_epoch: Id,
    connection_generation: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfStatus {
    scope: ShelfScope,
    reader_bundle: bool,
}

fn header_is(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    let values = headers.get_all(name);
    let mut values = values.iter();
    values.next().is_some_and(|value| value == expected) && values.next().is_none()
}

impl ShelfScope {
    pub(super) fn library_id(&self) -> Id {
        self.library_id
    }

    pub(super) fn subject_id(&self) -> Id {
        self.subject_id
    }

    pub(super) fn sync_epoch(&self) -> Id {
        self.sync_epoch
    }

    /// この接続（登録し直していない）か。
    pub(super) fn connection_is(&self, generation: u64) -> bool {
        self.connection_generation == generation
    }

    /// 同じ接続の同じ棚・利用者か。
    pub(super) fn same_as(&self, other: &Self) -> bool {
        self.library_id == other.library_id
            && self.subject_id == other.subject_id
            && self.sync_epoch == other.sync_epoch
            && self.connection_generation == other.connection_generation
    }

    #[cfg(test)]
    pub(super) fn for_tests(
        library_id: Id,
        subject_id: Id,
        sync_epoch: Id,
        connection_generation: u64,
    ) -> Self {
        Self {
            library_id,
            subject_id,
            sync_epoch,
            connection_generation,
        }
    }

    pub(super) fn matches(&self, headers: &HeaderMap) -> bool {
        header_is(
            headers,
            "x-piep-library-id",
            &self.library_id.get().to_string(),
        ) && header_is(
            headers,
            "x-piep-subject-id",
            &self.subject_id.get().to_string(),
        ) && header_is(
            headers,
            "x-piep-sync-epoch",
            &self.sync_epoch.get().to_string(),
        )
    }
}

async fn get(
    client: &reqwest::Client,
    saved: &SavedConnection,
    endpoint: &str,
    timeout: Duration,
) -> Result<Response, ClientError> {
    client
        .get(format!("{}{endpoint}", saved.url))
        .bearer_auth(&saved.token)
        .timeout(timeout)
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))
}

async fn parse_identity(
    response: Response,
    connection_generation: u64,
) -> Result<ShelfStatus, ClientError> {
    let response = check_response(response)?;
    let headers = response.headers().clone();
    let bytes = bounded_response_bytes(response, MAX_IDENTITY_BYTES).await?;
    let identity = LibraryIdentity::parse(&bytes).map_err(|_| ClientError::protocol())?;
    let scope = ShelfScope {
        library_id: identity.library_id,
        subject_id: identity.subject_id,
        sync_epoch: identity.sync_epoch,
        connection_generation,
    };
    if !scope.matches(&headers) {
        return Err(ClientError::protocol());
    }
    Ok(ShelfStatus {
        scope,
        reader_bundle: identity.capabilities.reader_bundle,
    })
}

pub(super) async fn discover(
    client: &reqwest::Client,
    saved: &SavedConnection,
    generation: u64,
) -> Option<ShelfStatus> {
    let response = get(client, saved, "/api/identity", PROBE_TIMEOUT)
        .await
        .ok()?;
    parse_identity(response, generation).await.ok()
}

pub(super) fn library_changed() -> ClientError {
    ClientError {
        code: "library_changed",
        message: "接続先の本棚が変わりました。一覧を開き直してください",
    }
}

pub(super) fn check_scope(response: Response, scope: ShelfScope) -> Result<Response, ClientError> {
    if response.status() == StatusCode::CONFLICT {
        return Err(library_changed());
    }
    // 401 は端末登録の解除と、棚だけの許可不足の両方を含む。
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(ClientError {
            code: "library_unavailable",
            message: "この端末には棚を読む許可がありません",
        });
    }
    let response = check_response(response)?;
    if !scope.matches(response.headers()) {
        return Err(library_changed());
    }
    Ok(response)
}

async fn read<T>(
    state: &PcClientState,
    scope: ShelfScope,
    endpoint: &str,
    parse: impl FnOnce(&[u8]) -> Result<T, ClientError>,
) -> Result<T, ClientError> {
    read_bounded(
        state,
        scope,
        endpoint,
        MAX_MANIFEST_BYTES,
        REQUEST_TIMEOUT,
        parse,
    )
    .await
}

async fn read_bounded<T>(
    state: &PcClientState,
    scope: ShelfScope,
    endpoint: &str,
    maximum: usize,
    timeout: Duration,
    parse: impl FnOnce(&[u8]) -> Result<T, ClientError>,
) -> Result<T, ClientError> {
    let (generation, saved) = state.connection_snapshot()?;
    if generation != scope.connection_generation {
        return Err(library_changed());
    }
    let saved = saved.ok_or_else(ClientError::invalid_pairing)?;
    let client = state.client(&saved)?;
    let response = get(&client, &saved, endpoint, timeout).await?;
    let bytes = bounded_response_bytes(check_scope(response, scope)?, maximum).await?;
    let result = parse(&bytes)?;
    state.with_connection(generation, || Ok(result))
}

/// 本文つきで問い合わせる（一覧の条件は長いので GET の query に載せない）。
async fn post_bounded<T>(
    state: &PcClientState,
    scope: ShelfScope,
    endpoint: &str,
    body: &serde_json::Value,
    parse: impl FnOnce(&[u8]) -> Result<T, ClientError>,
) -> Result<T, ClientError> {
    let (generation, saved) = state.connection_snapshot()?;
    if generation != scope.connection_generation {
        return Err(library_changed());
    }
    let saved = saved.ok_or_else(ClientError::invalid_pairing)?;
    let client = state.client(&saved)?;
    let response = client
        .post(format!("{}{endpoint}", saved.url))
        .bearer_auth(&saved.token)
        .timeout(REQUEST_TIMEOUT)
        .json(body)
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
    if response.status() == StatusCode::SERVICE_UNAVAILABLE {
        return Err(ClientError {
            code: "search_indexing",
            message: "サーバーは本文の検索の準備中です。しばらくしてからもう一度検索してください",
        });
    }
    let bytes = bounded_response_bytes(check_scope(response, scope)?, MAX_MANIFEST_BYTES).await?;
    let result = parse(&bytes)?;
    state.with_connection(generation, || Ok(result))
}

/// 十進の数字だけ（符号・空白・先頭の 0 の重ねを受け付けない）。
pub(super) fn canonical_generation(value: &str) -> bool {
    value
        .parse::<u64>()
        .is_ok_and(|parsed| parsed <= i64::MAX as u64 && parsed.to_string() == value)
}

/// お気に入りを付け外しした結果。`conflict` は別の端末やデスクトップで先に変わっていたとき
/// （値は今のもの。端末の時刻で勝者を決めない）。
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShelfFavorite {
    favorite: bool,
    generation: String,
    conflict: bool,
}

#[derive(Deserialize)]
struct FavoriteConflict {
    error: String,
    current: MobileShelfFavorite,
}

fn favorite_missing() -> ClientError {
    ClientError {
        code: "favorite_missing",
        message: "この作品は本棚から外されました。一覧を開き直してください",
    }
}

fn parse_favorite(bytes: &[u8], conflict: bool) -> Result<ShelfFavorite, ClientError> {
    let current = if conflict {
        let body: FavoriteConflict =
            serde_json::from_slice(bytes).map_err(|_| ClientError::protocol())?;
        if body.error != "favorite_conflict" {
            return Err(ClientError::protocol());
        }
        body.current
    } else {
        serde_json::from_slice::<MobileShelfFavorite>(bytes).map_err(|_| ClientError::protocol())?
    };
    if !canonical_generation(&current.generation) {
        return Err(ClientError::protocol());
    }
    Ok(ShelfFavorite {
        favorite: current.favorite,
        generation: current.generation,
        conflict,
    })
}

/// 棚の作品のお気に入りを付け外しする。`request` は画面が一度の操作ごとに作る UUID で、
/// 送り直しても一度だけ効く。`expected` は画面が知っている更新世代。
#[tauri::command]
pub async fn pc_set_favorite(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    work: Id,
    request: Id,
    expected: String,
    favorite: bool,
) -> Result<ShelfFavorite, ClientError> {
    if !canonical_generation(&expected) {
        return Err(ClientError::remote());
    }
    let (generation, saved) = state.connection_snapshot()?;
    if generation != library.connection_generation {
        return Err(library_changed());
    }
    let saved = saved.ok_or_else(ClientError::invalid_pairing)?;
    let client = state.client(&saved)?;
    let body = MobileShelfFavoriteUpdate {
        request_id: request.get().to_string(),
        expected,
        favorite,
    };
    let response = client
        .put(format!(
            "{}/api/libraries/{}/works/{}/favorite",
            saved.url,
            library.library_id.get(),
            work.get()
        ))
        .bearer_auth(&saved.token)
        .timeout(REQUEST_TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
    let result = favorite_response(response, library).await?;
    state.with_connection(generation, || Ok(result))
}

async fn favorite_response(
    response: Response,
    scope: ShelfScope,
) -> Result<ShelfFavorite, ClientError> {
    match response.status() {
        // 世代の食い違いは棚の入れ替わり（409）と同じ番号で返るので、本文で見分ける。
        StatusCode::CONFLICT if scope.matches(response.headers()) => {
            let bytes = bounded_response_bytes(response, MAX_IDENTITY_BYTES).await?;
            parse_favorite(&bytes, true).map_err(|_| library_changed())
        }
        StatusCode::NOT_FOUND => Err(favorite_missing()),
        _ => {
            let bytes =
                bounded_response_bytes(check_scope(response, scope)?, MAX_IDENTITY_BYTES).await?;
            parse_favorite(&bytes, false)
        }
    }
}

/// 一件が棚の作品として読めるか（UUID と取得元の鍵）。
fn valid_shelf_work(work: &MobileShelfWork) -> bool {
    uuid::Uuid::parse_str(&work.id).is_ok()
        && uuid::Uuid::parse_str(&work.snapshot_id).is_ok()
        && !work.source.is_empty()
        && work.source.len() <= 128
        && !work.source_id.is_empty()
        && work.source_id.len() <= 4096
}

/// 作品の一覧を棚から引く。条件はデスクトップの検索と同じ形に組んで送る（検査も同じ）。
#[tauri::command]
pub async fn pc_search(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    criteria: PcSearchCriteria,
    cursor: Option<String>,
    offset: Option<i64>,
    scope: Option<PcWorkScope>,
    limit: Option<i64>,
) -> Result<MobileShelfPage, ClientError> {
    if limit.is_some_and(|value| !(1..=50).contains(&value)) {
        return Err(ClientError::remote());
    }
    let mut payload = super::search_payload(&criteria, cursor, offset, scope)?;
    if let Some(limit) = limit {
        payload["limit"] = serde_json::json!(limit);
    }
    let endpoint = format!("/api/libraries/{}/works/search", library.library_id.get());
    post_bounded(&state, library, &endpoint, &payload, |bytes| {
        let page: MobileShelfPage =
            serde_json::from_slice(bytes).map_err(|_| ClientError::protocol())?;
        if page.items.len() > 50
            || !page.items.iter().all(valid_shelf_work)
            || page
                .next_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.is_empty() || cursor.len() > 4096)
        {
            return Err(ClientError::protocol());
        }
        Ok(page)
    })
    .await
}

/// 作品の画面（説明・作者とシリーズ・版）を棚から引く。
#[tauri::command]
pub async fn pc_work(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    work: Id,
) -> Result<MobileShelfWorkDetail, ClientError> {
    work_detail(&state, library, work).await
}

pub(super) async fn work_detail(
    state: &PcClientState,
    library: ShelfScope,
    work: Id,
) -> Result<MobileShelfWorkDetail, ClientError> {
    let endpoint = format!(
        "/api/libraries/{}/works/{}",
        library.library_id.get(),
        work.get()
    );
    read(state, library, &endpoint, |bytes| {
        let detail: MobileShelfWorkDetail =
            serde_json::from_slice(bytes).map_err(|_| ClientError::protocol())?;
        if detail.work.id != work.get().to_string() || !valid_shelf_work(&detail.work) {
            return Err(ClientError::protocol());
        }
        Ok(detail)
    })
    .await
}

#[tauri::command]
pub async fn pc_works(
    state: State<'_, PcClientState>,
    scope: ShelfScope,
    after: Option<String>,
) -> Result<ReaderWorkPage, ClientError> {
    let mut endpoint = format!("/api/libraries/{}/works", scope.library_id.get());
    if let Some(after) = after {
        if after.is_empty() || after.len() > 256 {
            return Err(ClientError::remote());
        }
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("after", &after)
            .finish();
        endpoint.push('?');
        endpoint.push_str(&query);
    }
    read(&state, scope, &endpoint, |bytes| {
        let page: ReaderWorkPage =
            serde_json::from_slice(bytes).map_err(|_| ClientError::protocol())?;
        if page.items.len() > 50
            || page
                .next
                .as_ref()
                .is_some_and(|after| after.is_empty() || after.len() > 256)
        {
            return Err(ClientError::protocol());
        }
        Ok(page)
    })
    .await
}

fn query(pairs: &[(&str, &str)]) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        query.append_pair(name, value);
    }
    query.finish()
}

fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ClientError> {
    serde_json::from_slice(bytes).map_err(|_| ClientError::protocol())
}

/// コレクションの ID は棚の UUID。表紙の経路にも使うので、ほかの形は受け取らない。
fn valid_collections(items: &[MobileCollection]) -> bool {
    items.len() <= 10_000
        && items
            .iter()
            .all(|item| uuid::Uuid::parse_str(&item.id).is_ok() && item.cover_tiles.len() <= 4)
}

fn kind_path(kind: &str) -> Result<&'static str, ClientError> {
    match kind {
        "people" => Ok("people"),
        "series" => Ok("series"),
        _ => Err(ClientError::remote()),
    }
}

/// 候補の種類。押した先を決めるので、知らない種類は受け取らない。
const SUGGESTION_KINDS: [&str; 4] = ["author", "series", "title", "tag"];

fn valid_suggestions(found: &MobileSearchSuggestions) -> bool {
    found.items.len() <= 12
        && found.items.iter().all(|item| {
            SUGGESTION_KINDS.contains(&item.kind.as_str())
                && !item.label.is_empty()
                && item.label.len() <= 4096
                && item.value.len() <= 4096
                && item.source.as_deref().is_none_or(|s| !s.is_empty() && s.len() <= 128)
                && item.source_key.as_deref().is_none_or(|k| !k.is_empty() && k.len() <= 4096)
                // 題の候補は棚の作品の ID で開く。
                && (item.kind != "title"
                    || item
                        .source_key
                        .as_deref()
                        .is_some_and(|key| uuid::Uuid::parse_str(key).is_ok()))
        })
}

/// 検索欄の候補を棚から引く。
#[tauri::command]
pub async fn pc_suggest(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    text: String,
) -> Result<MobileSearchSuggestions, ClientError> {
    if text.chars().count() > 200 {
        return Err(ClientError::remote());
    }
    let endpoint = format!(
        "/api/libraries/{}/works/suggest?{}",
        library.library_id.get(),
        query(&[("text", text.trim()), ("limit", "8")])
    );
    read(&state, library, &endpoint, |bytes| {
        let found: MobileSearchSuggestions = parse_json(bytes)?;
        if !valid_suggestions(&found) {
            return Err(ClientError::protocol());
        }
        Ok(found)
    })
    .await
}

/// 作者・シリーズの一覧（ライブラリの「作者」「シリーズ」）を棚から引く。
#[tauri::command]
pub async fn pc_entities(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    kind: String,
    text: String,
    offset: i64,
    sort: Option<String>,
    order: Option<String>,
) -> Result<MobileEntityPage, ClientError> {
    let kind = kind_path(&kind)?;
    if text.chars().count() > 200
        || !(0..=1_000_000).contains(&offset)
        || !super::valid_entity_sort(sort.as_deref(), order.as_deref())
    {
        return Err(ClientError::remote());
    }
    let offset = offset.to_string();
    let mut pairs = vec![
        ("text", text.trim()),
        ("offset", offset.as_str()),
        ("limit", "30"),
    ];
    if let Some(sort) = sort.as_deref() {
        pairs.push(("sort", sort));
    }
    if let Some(order) = order.as_deref() {
        pairs.push(("order", order));
    }
    let endpoint = format!(
        "/api/libraries/{}/{kind}?{}",
        library.library_id.get(),
        query(&pairs)
    );
    read(&state, library, &endpoint, |bytes| {
        let page: MobileEntityPage = parse_json(bytes)?;
        if page.items.len() > 50 {
            return Err(ClientError::protocol());
        }
        Ok(page)
    })
    .await
}

/// 作者・シリーズの画面の頭（説明・リンク・タグ・コレクション）を棚から引く。
#[tauri::command]
pub async fn pc_entity_detail(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    kind: String,
    source: String,
    key: String,
) -> Result<MobileEntityDetail, ClientError> {
    let kind = kind_path(&kind)?;
    if !super::valid_entity_identity(&source, &key) {
        return Err(ClientError::remote());
    }
    let endpoint = format!(
        "/api/libraries/{}/{kind}/detail?{}",
        library.library_id.get(),
        query(&[("provider", &source), ("key", &key)])
    );
    read(&state, library, &endpoint, |bytes| {
        let detail: MobileEntityDetail = parse_json(bytes)?;
        if detail.source != source
            || detail.source_key != key
            || !valid_collections(&detail.collections)
        {
            return Err(ClientError::protocol());
        }
        Ok(detail)
    })
    .await
}

/// 作者の作品が属するシリーズの一ページを棚から引く。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pc_entity_series(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    source: String,
    key: String,
    text: String,
    cursor: Option<String>,
    sort: Option<String>,
    order: Option<String>,
) -> Result<MobileEntitySeriesPage, ClientError> {
    if !super::valid_entity_identity(&source, &key)
        || text.chars().count() > 200
        || cursor.as_ref().is_some_and(|cursor| {
            cursor.is_empty() || cursor.len() > 7 || !cursor.bytes().all(|b| b.is_ascii_digit())
        })
        || !super::valid_entity_sort(sort.as_deref(), order.as_deref())
    {
        return Err(ClientError::remote());
    }
    let mut pairs = vec![
        ("provider", source.as_str()),
        ("key", key.as_str()),
        ("text", text.trim()),
        ("limit", "30"),
    ];
    for (name, value) in [("cursor", &cursor), ("sort", &sort), ("order", &order)] {
        if let Some(value) = value.as_deref() {
            pairs.push((name, value));
        }
    }
    let endpoint = format!(
        "/api/libraries/{}/people/series?{}",
        library.library_id.get(),
        query(&pairs)
    );
    read(&state, library, &endpoint, |bytes| {
        let page: MobileEntitySeriesPage = parse_json(bytes)?;
        if page.items.len() > 50
            || page
                .next_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.is_empty() || cursor.len() > 7)
        {
            return Err(ClientError::protocol());
        }
        Ok(page)
    })
    .await
}

/// コレクションの一覧を棚から引く。
#[tauri::command]
pub async fn pc_collections(
    state: State<'_, PcClientState>,
    library: ShelfScope,
) -> Result<Vec<MobileCollection>, ClientError> {
    let endpoint = format!("/api/libraries/{}/collections", library.library_id.get());
    read(&state, library, &endpoint, |bytes| {
        let items: Vec<MobileCollection> = parse_json(bytes)?;
        if !valid_collections(&items) {
            return Err(ClientError::protocol());
        }
        Ok(items)
    })
    .await
}

/// 作品の入ったコレクションと、その中の位置を棚から引く。
#[tauri::command]
pub async fn pc_work_collections(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    work: Id,
) -> Result<Vec<MobileCollection>, ClientError> {
    work_collections(&state, library, work).await
}

pub(super) async fn work_collections(
    state: &PcClientState,
    library: ShelfScope,
    work: Id,
) -> Result<Vec<MobileCollection>, ClientError> {
    let endpoint = format!(
        "/api/libraries/{}/works/{}/collections",
        library.library_id.get(),
        work.get()
    );
    read(state, library, &endpoint, |bytes| {
        let items: Vec<MobileCollection> = parse_json(bytes)?;
        if !valid_collections(&items) {
            return Err(ClientError::protocol());
        }
        Ok(items)
    })
    .await
}

/// コレクションの画面（回の作品は一覧と同じ棚の作品の形）を棚から引く。
#[tauri::command]
pub async fn pc_collection(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    collection: String,
) -> Result<MobileShelfCollectionDetail, ClientError> {
    // 棚のコレクションの UUID。サーバーが読み解くので、ここでは道に入れてよい文字かだけを見る。
    if collection.is_empty()
        || collection.len() > 128
        || !collection
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ClientError::remote());
    }
    let endpoint = format!(
        "/api/libraries/{}/collections/{collection}",
        library.library_id.get(),
    );
    read(&state, library, &endpoint, |bytes| {
        let detail: MobileShelfCollectionDetail = parse_json(bytes)?;
        if !valid_collections(std::slice::from_ref(&detail.summary))
            || !detail
                .members
                .iter()
                .all(|member| member.work.as_ref().is_none_or(valid_shelf_work))
        {
            return Err(ClientError::protocol());
        }
        Ok(detail)
    })
    .await
}

fn view_path(scope: ShelfScope, entity: Id, view: &ResourceScope) -> String {
    let (kind, id) = match view {
        ResourceScope::Snapshot(id) => ("snapshot", id),
        ResourceScope::Edit(id) => ("edit", id),
    };
    format!(
        "/api/libraries/{}/entities/{}/views/{kind}/{}",
        scope.library_id.get(),
        entity.get(),
        id.get(),
    )
}

/// 取得元と作品の鍵から、この利用者が今読む版。棚に無ければ誤り。
/// 取得元と作品の鍵から、棚の作品（一覧と同じ形）を引く。本文の中のリンクと、検索欄の題の
/// 候補から開くとき。棚に無い作品は 404 のまま返し、画面が他の手を探す。
#[tauri::command]
pub async fn pc_lookup_work(
    state: State<'_, PcClientState>,
    library: ShelfScope,
    provider: String,
    key: String,
) -> Result<MobileShelfWork, ClientError> {
    let found = lookup(&state, library, &provider, &key).await?;
    Ok(work_detail(&state, library, found.entity_id).await?.work)
}

pub(super) async fn lookup(
    state: &PcClientState,
    scope: ShelfScope,
    provider: &str,
    key: &str,
) -> Result<WorkReadingView, ClientError> {
    if provider.is_empty() || provider.len() > 128 || key.is_empty() || key.len() > 4096 {
        return Err(ClientError::remote());
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("provider", provider)
        .append_pair("key", key)
        .finish();
    let endpoint = format!(
        "/api/libraries/{}/works/lookup?{query}",
        scope.library_id.get()
    );
    read(state, scope, &endpoint, |bytes| {
        serde_json::from_slice(bytes).map_err(|_| ClientError::protocol())
    })
    .await
}

pub(super) async fn bundle(
    state: &PcClientState,
    scope: ShelfScope,
    entity: Id,
    view: ResourceScope,
) -> Result<ReaderBundle, ClientError> {
    let endpoint = format!("{}/bundle", view_path(scope, entity, &view));
    read(state, scope, &endpoint, |bytes| {
        let bundle = ReaderBundle::parse(bytes).map_err(|_| ClientError::protocol())?;
        if bundle.library_id != scope.library_id
            || bundle.entity_id != entity
            || bundle.scope != view
        {
            return Err(ClientError::protocol());
        }
        Ok(bundle)
    })
    .await
}

/// 資産の部分実体を、保存したときの圧縮のまま受け取る。中身の確認は呼び手が行う。
pub(super) async fn chunk(
    state: &PcClientState,
    scope: ShelfScope,
    entity: Id,
    view: &ResourceScope,
    resource: &ResourceReference,
    file: Fingerprint,
    ordinal: usize,
) -> Result<Vec<u8>, ClientError> {
    let (kind, slot) = match resource {
        ResourceReference::Media { slot } => ("media", slot),
        ResourceReference::File { slot } => ("file", slot),
        _ => return Err(ClientError::remote()),
    };
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("slot", slot)
        .finish();
    let endpoint = format!(
        "{}/resources/{kind}/files/{file}/chunks/{ordinal}?{query}",
        view_path(scope, entity, view)
    );
    read_bounded(
        state,
        scope,
        &endpoint,
        piep_storage::MAX_ENCODED_CHUNK_BYTES as usize,
        super::ASSET_TIMEOUT,
        |bytes| Ok(bytes.to_vec()),
    )
    .await
}

#[tauri::command]
pub async fn pc_bundle(
    state: State<'_, PcClientState>,
    scope: ShelfScope,
    entity: Id,
    view: ResourceScope,
) -> Result<ReaderBundle, ClientError> {
    bundle(&state, scope, entity, view).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pc_client::tests::local_response;

    fn identity() -> LibraryIdentity {
        let id = || Id::new(uuid::Uuid::new_v4()).unwrap();
        LibraryIdentity::new(id(), id(), id())
    }

    fn headers(identity: &LibraryIdentity) -> String {
        format!(
            "X-Piep-Library-Id: {}\r\nX-Piep-Subject-Id: {}\r\nX-Piep-Sync-Epoch: {}\r\n",
            identity.library_id.get(),
            identity.subject_id.get(),
            identity.sync_epoch.get(),
        )
    }

    async fn response(status: &str, headers: &str, body: &str) -> Response {
        local_response(format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            body.len(),
        ))
        .await
    }

    #[test]
    fn shelf_identity_requires_the_wire_format_and_confirmed_header_scope() {
        tauri::async_runtime::block_on(async {
            let shelf = identity();
            let wire_headers = headers(&shelf);
            let body = serde_json::to_string(&shelf).unwrap();
            let connected = parse_identity(response("200 OK", &wire_headers, &body).await, 7)
                .await
                .unwrap();
            assert_eq!(connected.scope.library_id, shelf.library_id);
            assert_eq!(connected.scope.subject_id, shelf.subject_id);
            assert_eq!(connected.scope.sync_epoch, shelf.sync_epoch);
            assert_eq!(connected.scope.connection_generation, 7);
            assert!(connected.reader_bundle);
            let duplicate = format!(
                "{wire_headers}X-Piep-Sync-Epoch: {}\r\n",
                shelf.sync_epoch.get()
            );
            for (head, content) in [
                (headers(&identity()), body.as_str()),
                (duplicate, body.as_str()),
                (wire_headers, r#"{"apiVersion":2,"serverName":"piep"}"#),
            ] {
                assert_eq!(
                    parse_identity(response("200 OK", &head, content).await, 7)
                        .await
                        .unwrap_err()
                        .code,
                    "protocol_mismatch",
                );
            }
        });
    }

    #[test]
    fn following_reads_reject_a_changed_scope_and_separate_permission_from_revocation() {
        tauri::async_runtime::block_on(async {
            let shelf = identity();
            let scope = ShelfScope {
                library_id: shelf.library_id,
                subject_id: shelf.subject_id,
                sync_epoch: shelf.sync_epoch,
                connection_generation: 1,
            };
            let wire_headers = headers(&shelf);
            assert!(check_scope(response("200 OK", &wire_headers, "").await, scope).is_ok());
            let changed_epoch =
                LibraryIdentity::new(shelf.library_id, shelf.subject_id, identity().sync_epoch);
            for (status, head, code) in [
                ("200 OK", headers(&changed_epoch), "library_changed"),
                ("409 Conflict", wire_headers, "library_changed"),
                ("401 Unauthorized", String::new(), "library_unavailable"),
            ] {
                assert_eq!(
                    check_scope(response(status, &head, "").await, scope)
                        .unwrap_err()
                        .code,
                    code,
                );
            }
        });
    }

    #[test]
    fn a_favorite_conflict_returns_the_current_value_and_a_changed_shelf_stays_a_change() {
        tauri::async_runtime::block_on(async {
            let shelf = identity();
            let scope = ShelfScope {
                library_id: shelf.library_id,
                subject_id: shelf.subject_id,
                sync_epoch: shelf.sync_epoch,
                connection_generation: 1,
            };
            let wire = headers(&shelf);
            let stored = favorite_response(
                response("200 OK", &wire, r#"{"favorite":true,"generation":"3"}"#).await,
                scope,
            )
            .await
            .unwrap();
            assert_eq!(
                stored,
                ShelfFavorite {
                    favorite: true,
                    generation: "3".into(),
                    conflict: false
                }
            );
            let conflict = favorite_response(
                response(
                    "409 Conflict",
                    &wire,
                    r#"{"error":"favorite_conflict","current":{"favorite":false,"generation":"4"}}"#,
                )
                .await,
                scope,
            )
            .await
            .unwrap();
            assert_eq!(
                (
                    conflict.favorite,
                    conflict.generation.as_str(),
                    conflict.conflict
                ),
                (false, "4", true)
            );
            let other = headers(&LibraryIdentity::new(
                shelf.library_id,
                shelf.subject_id,
                identity().sync_epoch,
            ));
            for (status, head, body, code) in [
                (
                    "409 Conflict",
                    other.clone(),
                    r#"{"error":"favorite_conflict","current":{"favorite":false,"generation":"4"}}"#,
                    "library_changed",
                ),
                (
                    "409 Conflict",
                    wire.clone(),
                    r#"{"error":"library_changed"}"#,
                    "library_changed",
                ),
                ("404 Not Found", wire.clone(), "", "favorite_missing"),
                (
                    "200 OK",
                    wire.clone(),
                    r#"{"favorite":true,"generation":"03"}"#,
                    "protocol_mismatch",
                ),
                (
                    "200 OK",
                    other,
                    r#"{"favorite":true,"generation":"3"}"#,
                    "library_changed",
                ),
            ] {
                assert_eq!(
                    favorite_response(response(status, &head, body).await, scope)
                        .await
                        .unwrap_err()
                        .code,
                    code,
                    "{status} {body}"
                );
            }
            assert!(canonical_generation("0") && canonical_generation("12"));
            assert!(
                !canonical_generation("-1")
                    && !canonical_generation("01")
                    && !canonical_generation(" 1")
            );
        });
    }

    #[test]
    fn suggestions_refuse_unknown_kinds_and_titles_without_a_shelf_work() {
        let item = |kind: &str, key: Option<&str>| piep_mobile_api::MobileSearchSuggestion {
            kind: kind.into(),
            label: "名".into(),
            value: "1".into(),
            count: Some(1),
            exact_match: false,
            source: Some("pixiv".into()),
            source_key: key.map(Into::into),
        };
        let found = |items| MobileSearchSuggestions { items };
        let work = "0190a000-0000-7000-8000-000000000002";
        assert!(valid_suggestions(&found(vec![
            item("author", Some("7")),
            item("series", Some("9")),
            item("title", Some(work)),
            item("tag", None),
        ])));
        assert!(!valid_suggestions(&found(vec![item("title", Some("42"))])));
        assert!(!valid_suggestions(&found(vec![item("title", None)])));
        assert!(!valid_suggestions(&found(vec![item("other", None)])));
        assert!(!valid_suggestions(&found(vec![item("tag", None); 13])));
    }
}
