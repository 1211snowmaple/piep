//! 新しい棚の接続情報と読書用の応答を、登録した端末の認証で受け取る。
use super::{
    ClientError, PROBE_TIMEOUT, PcClientState, REQUEST_TIMEOUT, SavedConnection,
    bounded_response_bytes, check_response,
};
use piep_library_protocol::{
    Id, LibraryIdentity, MAX_IDENTITY_BYTES, MAX_MANIFEST_BYTES, ReaderBundle, ReaderWorkPage,
    ResourceReference, ResourceScope, WorkReadingView,
};
use piep_storage::Fingerprint;
use reqwest::{Response, StatusCode, header::HeaderMap};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::State;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeLibraryScope {
    library_id: Id,
    subject_id: Id,
    sync_epoch: Id,
    connection_generation: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeLibraryStatus {
    scope: NativeLibraryScope,
    works_list: bool,
    reader_bundle: bool,
}

fn header_is(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    let values = headers.get_all(name);
    let mut values = values.iter();
    values.next().is_some_and(|value| value == expected) && values.next().is_none()
}

impl NativeLibraryScope {
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

    fn matches(&self, headers: &HeaderMap) -> bool {
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
) -> Result<NativeLibraryStatus, ClientError> {
    let response = check_response(response)?;
    let headers = response.headers().clone();
    let bytes = bounded_response_bytes(response, MAX_IDENTITY_BYTES).await?;
    let identity = LibraryIdentity::parse(&bytes).map_err(|_| ClientError::protocol())?;
    let scope = NativeLibraryScope {
        library_id: identity.library_id,
        subject_id: identity.subject_id,
        sync_epoch: identity.sync_epoch,
        connection_generation,
    };
    if !scope.matches(&headers) {
        return Err(ClientError::protocol());
    }
    Ok(NativeLibraryStatus {
        scope,
        works_list: header_is(&headers, "x-piep-works-list", "1"),
        reader_bundle: identity.capabilities.reader_bundle,
    })
}

pub(super) async fn discover(
    client: &reqwest::Client,
    saved: &SavedConnection,
    generation: u64,
) -> Option<NativeLibraryStatus> {
    let response = get(client, saved, "/api/v2/identity", PROBE_TIMEOUT)
        .await
        .ok()?;
    parse_identity(response, generation).await.ok()
}

fn library_changed() -> ClientError {
    ClientError {
        code: "library_changed",
        message: "接続先の本棚が変わりました。一覧を開き直してください",
    }
}

fn check_scope(response: Response, scope: NativeLibraryScope) -> Result<Response, ClientError> {
    if response.status() == StatusCode::CONFLICT {
        return Err(library_changed());
    }
    // v2 の 401 は端末登録の解除と、新しい棚だけの許可不足の両方を含む。
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(ClientError {
            code: "library_unavailable",
            message: "この端末には新しい棚を読む許可がありません",
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
    scope: NativeLibraryScope,
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
    scope: NativeLibraryScope,
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

#[tauri::command]
pub async fn pc_native_works(
    state: State<'_, PcClientState>,
    scope: NativeLibraryScope,
    after: Option<String>,
) -> Result<ReaderWorkPage, ClientError> {
    let mut endpoint = format!("/api/v2/libraries/{}/works", scope.library_id.get());
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

fn view_path(scope: NativeLibraryScope, entity: Id, view: &ResourceScope) -> String {
    let (kind, id) = match view {
        ResourceScope::Snapshot(id) => ("snapshot", id),
        ResourceScope::Edit(id) => ("edit", id),
    };
    format!(
        "/api/v2/libraries/{}/entities/{}/views/{kind}/{}",
        scope.library_id.get(),
        entity.get(),
        id.get(),
    )
}

/// 取得元と作品の鍵から、この利用者が今読む版。棚に無ければ誤り。
pub(super) async fn lookup(
    state: &PcClientState,
    scope: NativeLibraryScope,
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
        "/api/v2/libraries/{}/works/lookup?{query}",
        scope.library_id.get()
    );
    read(state, scope, &endpoint, |bytes| {
        serde_json::from_slice(bytes).map_err(|_| ClientError::protocol())
    })
    .await
}

pub(super) async fn bundle(
    state: &PcClientState,
    scope: NativeLibraryScope,
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
    scope: NativeLibraryScope,
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
pub async fn pc_native_bundle(
    state: State<'_, PcClientState>,
    scope: NativeLibraryScope,
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
    fn native_identity_requires_the_wire_format_and_confirmed_header_scope() {
        tauri::async_runtime::block_on(async {
            let shelf = identity();
            let wire_headers = headers(&shelf);
            let body = serde_json::to_string(&shelf).unwrap();
            let advertised = format!("{wire_headers}X-Piep-Works-List: 1\r\n");
            let connected = parse_identity(response("200 OK", &advertised, &body).await, 7)
                .await
                .unwrap();
            assert_eq!(connected.scope.library_id, shelf.library_id);
            assert_eq!(connected.scope.subject_id, shelf.subject_id);
            assert_eq!(connected.scope.sync_epoch, shelf.sync_epoch);
            assert_eq!(connected.scope.connection_generation, 7);
            assert!(connected.works_list && connected.reader_bundle);
            let older = parse_identity(response("200 OK", &wire_headers, &body).await, 7)
                .await
                .unwrap();
            assert!(!older.works_list);
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
    fn following_reads_reject_a_changed_scope_and_separate_permission_from_v1_revocation() {
        tauri::async_runtime::block_on(async {
            let shelf = identity();
            let scope = NativeLibraryScope {
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
}
