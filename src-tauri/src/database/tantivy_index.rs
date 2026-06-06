use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value as _;
use tantivy::schema::{Field, Schema, FAST, INDEXED, STORED, STRING, TEXT};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, RemoveLongFilter, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

use super::search::{parse_search_query, ParsedSearchQuery, SearchDocument, SearchTerm};
use super::search_normalization::{
    expand_query_for_tantivy, normalize_for_search, query_variants, reading_kana_text,
    reading_romaji_text, search_index_text,
};

const INDEX_DIR_NAME: &str = "search-index";
const INDEX_VERSION_DIR: &str = "v3";
const TOKENIZER_NAME: &str = "default";

static RUNTIMES: OnceLock<Mutex<HashMap<PathBuf, Arc<TantivyRuntime>>>> = OnceLock::new();

struct TantivyRuntime {
    index: Index,
    reader: IndexReader,
    fields: TantivyFields,
}

#[derive(Debug, Clone)]
pub struct TantivyIndexDocument {
    pub download_id: i64,
    pub source: String,
    pub source_id: String,
    pub source_url: String,
    pub title: String,
    pub author_name: String,
    pub author_id: String,
    pub tags: String,
    pub series_title: String,
    pub excerpt: String,
    pub body: String,
    pub published_at: String,
    pub downloaded_at: String,
    pub favorite: bool,
    pub watch_updates: bool,
    pub asset_kinds: String,
    pub text_length: i64,
}

#[derive(Debug, Clone)]
pub struct TantivySearchHit {
    pub download_id: i64,
    pub score: f32,
}

#[derive(Clone, Copy)]
struct TantivyFields {
    download_id: Field,
    source_exact: Field,
    source_id_exact: Field,
    source_id_lower: Field,
    source_id_ngram: Field,
    source_id_reading_kana: Field,
    source_id_reading_romaji: Field,
    source_url_exact: Field,
    source_url_lower: Field,
    source_url_ngram: Field,
    source_url_reading_kana: Field,
    source_url_reading_romaji: Field,
    title_exact: Field,
    title_lower: Field,
    title_ngram: Field,
    title_reading_kana: Field,
    title_reading_romaji: Field,
    author_name_exact: Field,
    author_name_lower: Field,
    author_name_ngram: Field,
    author_name_reading_kana: Field,
    author_name_reading_romaji: Field,
    author_id_exact: Field,
    author_id_lower: Field,
    author_id_ngram: Field,
    author_id_reading_kana: Field,
    author_id_reading_romaji: Field,
    tags_exact: Field,
    tags_lower: Field,
    tags_ngram: Field,
    tags_reading_kana: Field,
    tags_reading_romaji: Field,
    series_title_exact: Field,
    series_title_lower: Field,
    series_title_ngram: Field,
    series_title_reading_kana: Field,
    series_title_reading_romaji: Field,
    excerpt_exact: Field,
    excerpt_lower: Field,
    excerpt_ngram: Field,
    excerpt_reading_kana: Field,
    excerpt_reading_romaji: Field,
    body_exact: Field,
    body_lower: Field,
    body_ngram: Field,
    body_reading_kana: Field,
    body_reading_romaji: Field,
    body_chunk: Field,
    published_at: Field,
    downloaded_at: Field,
    favorite: Field,
    watch_updates: Field,
    asset_kinds: Field,
    text_length: Field,
}

pub fn ensure_index(storage_dir: &Path) -> Result<Index, String> {
    let index_dir = storage_dir.join(INDEX_DIR_NAME).join(INDEX_VERSION_DIR);
    std::fs::create_dir_all(&index_dir)
        .map_err(|e| format!("Tantivy index dir creation failed: {}", e))?;

    match Index::open_in_dir(&index_dir) {
        Ok(index) => {
            register_tokenizer(&index)?;
            if schema_has_required_fields(&index.schema()) {
                Ok(index)
            } else {
                reset_index_dir(&index_dir)
            }
        }
        Err(_) => reset_index_dir(&index_dir),
    }
}

fn reset_index_dir(index_dir: &Path) -> Result<Index, String> {
    if index_dir.exists() {
        let _ = std::fs::remove_dir_all(index_dir);
    }
    std::fs::create_dir_all(index_dir).map_err(|e| format!("Tantivy index reset failed: {}", e))?;
    let index = Index::create_in_dir(index_dir, build_schema())
        .map_err(|e| format!("Tantivy index creation failed: {}", e))?;
    register_tokenizer(&index)?;
    Ok(index)
}

fn runtime(storage_dir: &Path) -> Result<Arc<TantivyRuntime>, String> {
    let key = storage_dir.join(INDEX_DIR_NAME).join(INDEX_VERSION_DIR);
    let runtimes = RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(runtime) = runtimes.lock().get(&key).cloned() {
        return Ok(runtime);
    }

    let index = ensure_index(storage_dir)?;
    let schema = index.schema();
    let fields = fields(&schema)?;
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::OnCommitWithDelay)
        .try_into()
        .map_err(|e| format!("Tantivy reader creation failed: {}", e))?;
    let runtime = Arc::new(TantivyRuntime {
        index,
        reader,
        fields,
    });
    runtimes.lock().insert(key, runtime.clone());
    Ok(runtime)
}

pub fn upsert_documents(storage_dir: &Path, docs: &[TantivyIndexDocument]) -> Result<(), String> {
    if docs.is_empty() {
        return Ok(());
    }
    let runtime = runtime(storage_dir)?;
    let mut writer = runtime
        .index
        .writer(64_000_000)
        .map_err(|e| format!("Tantivy writer creation failed: {}", e))?;
    for doc in docs {
        writer.delete_term(Term::from_field_u64(
            runtime.fields.download_id,
            doc.download_id as u64,
        ));
        writer
            .add_document(tantivy_document(&runtime.fields, doc))
            .map_err(|e| format!("Tantivy document insert failed: {}", e))?;
    }
    writer
        .commit()
        .map_err(|e| format!("Tantivy commit failed: {}", e))?;
    runtime
        .reader
        .reload()
        .map_err(|e| format!("Tantivy reader reload failed: {}", e))?;
    Ok(())
}

pub fn delete_document(storage_dir: &Path, download_id: i64) -> Result<(), String> {
    let runtime = runtime(storage_dir)?;
    let mut writer: IndexWriter<TantivyDocument> = runtime
        .index
        .writer(32_000_000)
        .map_err(|e| format!("Tantivy writer creation failed: {}", e))?;
    writer.delete_term(Term::from_field_u64(
        runtime.fields.download_id,
        download_id as u64,
    ));
    writer
        .commit()
        .map_err(|e| format!("Tantivy delete commit failed: {}", e))?;
    runtime
        .reader
        .reload()
        .map_err(|e| format!("Tantivy reader reload failed: {}", e))?;
    Ok(())
}

pub fn search(
    storage_dir: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<TantivySearchHit>, String> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let runtime = runtime(storage_dir)?;
    let fields = runtime.fields;
    let searcher = runtime.reader.searcher();
    let mut parser = QueryParser::for_index(&runtime.index, search_fields(&fields));
    configure_field_boosts(&mut parser, &fields);
    let parsed_query = parse_search_query(query);
    let Some(query_expr) = tantivy_query(&parsed_query) else {
        return Ok(Vec::new());
    };
    let expanded_query = expand_query_for_tantivy(query);
    let parsed = parser
        .parse_query(&query_expr)
        .or_else(|_| parser.parse_query(&escape_query(&expanded_query)))
        .map_err(|e| format!("Tantivy query parse failed: {}", e))?;

    let top_docs = searcher
        .search(&parsed, &TopDocs::with_limit(limit).order_by_score())
        .map_err(|e| format!("Tantivy search failed: {}", e))?;

    let mut hits = Vec::with_capacity(top_docs.len());
    for (score, address) in top_docs {
        let retrieved = searcher
            .doc::<TantivyDocument>(address)
            .map_err(|e| format!("Tantivy document read failed: {}", e))?;
        if let Some(download_id) = retrieved
            .get_first(fields.download_id)
            .and_then(|value| value.as_u64())
            .map(|value| value as i64)
        {
            hits.push(TantivySearchHit { download_id, score });
        }
    }

    Ok(hits)
}

fn search_fields(fields: &TantivyFields) -> Vec<Field> {
    vec![
        fields.title_exact,
        fields.title_lower,
        fields.title_ngram,
        fields.title_reading_kana,
        fields.title_reading_romaji,
        fields.author_name_exact,
        fields.author_name_lower,
        fields.author_name_ngram,
        fields.author_name_reading_kana,
        fields.author_name_reading_romaji,
        fields.author_id_exact,
        fields.author_id_lower,
        fields.author_id_ngram,
        fields.author_id_reading_kana,
        fields.author_id_reading_romaji,
        fields.source_id_exact,
        fields.source_id_lower,
        fields.source_id_ngram,
        fields.source_id_reading_kana,
        fields.source_id_reading_romaji,
        fields.source_url_exact,
        fields.source_url_lower,
        fields.source_url_ngram,
        fields.source_url_reading_kana,
        fields.source_url_reading_romaji,
        fields.tags_exact,
        fields.tags_lower,
        fields.tags_ngram,
        fields.tags_reading_kana,
        fields.tags_reading_romaji,
        fields.series_title_exact,
        fields.series_title_lower,
        fields.series_title_ngram,
        fields.series_title_reading_kana,
        fields.series_title_reading_romaji,
        fields.excerpt_exact,
        fields.excerpt_lower,
        fields.excerpt_ngram,
        fields.excerpt_reading_kana,
        fields.excerpt_reading_romaji,
        fields.body_lower,
        fields.body_ngram,
        fields.body_reading_kana,
        fields.body_reading_romaji,
        fields.body_chunk,
        fields.asset_kinds,
    ]
}

fn configure_field_boosts(parser: &mut QueryParser, fields: &TantivyFields) {
    for (field, boost) in [
        (fields.source_id_exact, 10.0),
        (fields.source_id_lower, 9.0),
        (fields.source_id_ngram, 8.0),
        (fields.source_url_exact, 9.0),
        (fields.source_url_lower, 8.0),
        (fields.source_url_ngram, 7.0),
        (fields.title_exact, 12.0),
        (fields.title_lower, 11.0),
        (fields.title_ngram, 10.0),
        (fields.title_reading_kana, 9.0),
        (fields.title_reading_romaji, 8.5),
        (fields.author_name_exact, 10.0),
        (fields.author_name_lower, 9.5),
        (fields.author_name_ngram, 8.5),
        (fields.author_name_reading_kana, 8.0),
        (fields.author_name_reading_romaji, 7.5),
        (fields.author_id_exact, 9.0),
        (fields.author_id_lower, 8.0),
        (fields.author_id_ngram, 7.0),
        (fields.tags_exact, 9.0),
        (fields.tags_lower, 8.5),
        (fields.tags_ngram, 8.0),
        (fields.tags_reading_kana, 7.5),
        (fields.tags_reading_romaji, 7.0),
        (fields.series_title_exact, 8.0),
        (fields.series_title_lower, 7.5),
        (fields.series_title_ngram, 7.0),
        (fields.series_title_reading_kana, 6.5),
        (fields.series_title_reading_romaji, 6.0),
        (fields.excerpt_exact, 4.0),
        (fields.excerpt_lower, 3.8),
        (fields.excerpt_ngram, 3.5),
        (fields.excerpt_reading_kana, 3.2),
        (fields.excerpt_reading_romaji, 3.0),
        (fields.body_lower, 1.2),
        (fields.body_ngram, 1.0),
        (fields.body_reading_kana, 0.9),
        (fields.body_reading_romaji, 0.8),
        (fields.body_chunk, 0.7),
        (fields.asset_kinds, 1.0),
    ] {
        parser.set_field_boost(field, boost);
    }
}

fn tantivy_query(parsed: &ParsedSearchQuery) -> Option<String> {
    if parsed.include.is_empty() {
        return None;
    }

    let mut parts = parsed
        .include
        .iter()
        .filter_map(term_query_group)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }

    for term in &parsed.exclude {
        if let Some(group) = term_query_group(term) {
            parts.push(format!("-{}", group));
        }
    }

    Some(parts.join(" AND "))
}

fn term_query_group(term: &SearchTerm) -> Option<String> {
    let mut variants = Vec::new();
    for variant in &term.variants {
        push_query_variant(&mut variants, variant);
    }
    for synonym in &term.synonyms {
        for variant in query_variants(synonym) {
            push_query_variant(&mut variants, &variant);
        }
    }

    if variants.is_empty() {
        None
    } else if variants.len() == 1 {
        Some(variants.remove(0))
    } else {
        Some(format!("({})", variants.join(" OR ")))
    }
}

fn push_query_variant(values: &mut Vec<String>, value: &str) {
    let value = query_literal(value);
    if value.is_empty() || values.iter().any(|existing| existing == &value) {
        return;
    }
    values.push(value);
}

fn query_literal(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    if value.chars().any(char::is_whitespace) {
        return format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""));
    }

    let mut out = String::new();
    for c in value.chars() {
        if matches!(
            c,
            '+' | '^'
                | '`'
                | '{'
                | '}'
                | '['
                | ']'
                | '"'
                | '~'
                | '*'
                | '?'
                | ':'
                | '\\'
                | '/'
                | '('
                | ')'
                | '-'
                | '!'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn load_document(
    storage_dir: &Path,
    download_id: i64,
) -> Result<Option<SearchDocument>, String> {
    let runtime = runtime(storage_dir)?;
    let fields = runtime.fields;
    let searcher = runtime.reader.searcher();
    let term = Term::from_field_u64(fields.download_id, download_id as u64);
    let query = tantivy::query::TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
    let top_docs = searcher
        .search(
            &query as &dyn tantivy::query::Query,
            &TopDocs::with_limit(1).order_by_score(),
        )
        .map_err(|e| format!("Tantivy document lookup failed: {}", e))?;
    let Some((_, address)) = top_docs.into_iter().next() else {
        return Ok(None);
    };
    let retrieved = searcher
        .doc::<TantivyDocument>(address)
        .map_err(|e| format!("Tantivy document read failed: {}", e))?;

    Ok(Some(SearchDocument {
        title: text_value(&retrieved, fields.title_exact),
        author_name: text_value(&retrieved, fields.author_name_exact),
        tags: text_value(&retrieved, fields.tags_exact),
        series_title: text_value(&retrieved, fields.series_title_exact),
        excerpt: text_value(&retrieved, fields.excerpt_exact),
        body: text_value(&retrieved, fields.body_exact),
    }))
}

fn tantivy_document(fields: &TantivyFields, doc: &TantivyIndexDocument) -> TantivyDocument {
    let mut tantivy_doc = TantivyDocument::default();
    tantivy_doc.add_u64(fields.download_id, doc.download_id as u64);
    tantivy_doc.add_text(fields.source_exact, &doc.source);
    add_search_text(
        &mut tantivy_doc,
        fields.source_id_exact,
        fields.source_id_lower,
        fields.source_id_ngram,
        fields.source_id_reading_kana,
        fields.source_id_reading_romaji,
        &doc.source_id,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.source_url_exact,
        fields.source_url_lower,
        fields.source_url_ngram,
        fields.source_url_reading_kana,
        fields.source_url_reading_romaji,
        &doc.source_url,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.title_exact,
        fields.title_lower,
        fields.title_ngram,
        fields.title_reading_kana,
        fields.title_reading_romaji,
        &doc.title,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.author_name_exact,
        fields.author_name_lower,
        fields.author_name_ngram,
        fields.author_name_reading_kana,
        fields.author_name_reading_romaji,
        &doc.author_name,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.author_id_exact,
        fields.author_id_lower,
        fields.author_id_ngram,
        fields.author_id_reading_kana,
        fields.author_id_reading_romaji,
        &doc.author_id,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.tags_exact,
        fields.tags_lower,
        fields.tags_ngram,
        fields.tags_reading_kana,
        fields.tags_reading_romaji,
        &doc.tags,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.series_title_exact,
        fields.series_title_lower,
        fields.series_title_ngram,
        fields.series_title_reading_kana,
        fields.series_title_reading_romaji,
        &doc.series_title,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.excerpt_exact,
        fields.excerpt_lower,
        fields.excerpt_ngram,
        fields.excerpt_reading_kana,
        fields.excerpt_reading_romaji,
        &doc.excerpt,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.body_exact,
        fields.body_lower,
        fields.body_ngram,
        fields.body_reading_kana,
        fields.body_reading_romaji,
        &doc.body,
    );
    tantivy_doc.add_text(fields.body_chunk, search_index_text(&doc.body));
    tantivy_doc.add_text(fields.published_at, &doc.published_at);
    tantivy_doc.add_text(fields.downloaded_at, &doc.downloaded_at);
    tantivy_doc.add_bool(fields.favorite, doc.favorite);
    tantivy_doc.add_bool(fields.watch_updates, doc.watch_updates);
    tantivy_doc.add_text(fields.asset_kinds, &doc.asset_kinds);
    tantivy_doc.add_i64(fields.text_length, doc.text_length);
    tantivy_doc
}

fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_u64_field("download_id", INDEXED | STORED);
    builder.add_text_field("source_exact", STRING | STORED);
    add_text_family(&mut builder, "source_id");
    add_text_family(&mut builder, "source_url");
    add_text_family(&mut builder, "title");
    add_text_family(&mut builder, "author_name");
    add_text_family(&mut builder, "author_id");
    add_text_family(&mut builder, "tags");
    add_text_family(&mut builder, "series_title");
    add_text_family(&mut builder, "excerpt");
    add_text_family(&mut builder, "body");
    builder.add_text_field("body_chunk", TEXT | STORED);
    builder.add_text_field("published_at", STRING | STORED);
    builder.add_text_field("downloaded_at", STRING | STORED);
    builder.add_bool_field("favorite", FAST | STORED);
    builder.add_bool_field("watch_updates", FAST | STORED);
    builder.add_text_field("asset_kinds", TEXT | STORED);
    builder.add_i64_field("text_length", FAST | STORED);
    builder.build()
}

fn add_text_family(builder: &mut tantivy::schema::SchemaBuilder, name: &str) {
    builder.add_text_field(&format!("{}_exact", name), STRING | STORED);
    builder.add_text_field(&format!("{}_lower", name), STRING | STORED);
    builder.add_text_field(&format!("{}_ngram", name), TEXT | STORED);
    builder.add_text_field(&format!("{}_reading_kana", name), TEXT | STORED);
    builder.add_text_field(&format!("{}_reading_romaji", name), TEXT | STORED);
}

fn schema_has_required_fields(schema: &Schema) -> bool {
    [
        "download_id",
        "source_exact",
        "source_id_exact",
        "source_id_lower",
        "source_id_ngram",
        "source_id_reading_kana",
        "source_id_reading_romaji",
        "source_url_exact",
        "source_url_lower",
        "source_url_ngram",
        "source_url_reading_kana",
        "source_url_reading_romaji",
        "title_exact",
        "title_lower",
        "title_ngram",
        "title_reading_kana",
        "title_reading_romaji",
        "author_name_exact",
        "author_name_lower",
        "author_name_ngram",
        "author_name_reading_kana",
        "author_name_reading_romaji",
        "author_id_exact",
        "author_id_lower",
        "author_id_ngram",
        "author_id_reading_kana",
        "author_id_reading_romaji",
        "tags_exact",
        "tags_lower",
        "tags_ngram",
        "tags_reading_kana",
        "tags_reading_romaji",
        "series_title_exact",
        "series_title_lower",
        "series_title_ngram",
        "series_title_reading_kana",
        "series_title_reading_romaji",
        "excerpt_exact",
        "excerpt_lower",
        "excerpt_ngram",
        "excerpt_reading_kana",
        "excerpt_reading_romaji",
        "body_exact",
        "body_lower",
        "body_ngram",
        "body_reading_kana",
        "body_reading_romaji",
        "body_chunk",
        "published_at",
        "downloaded_at",
        "favorite",
        "watch_updates",
        "asset_kinds",
        "text_length",
    ]
    .iter()
    .all(|field| schema.get_field(field).is_ok())
}

fn register_tokenizer(index: &Index) -> Result<(), String> {
    let tokenizer = TextAnalyzer::builder(
        NgramTokenizer::all_ngrams(2, 3)
            .map_err(|e| format!("Tantivy tokenizer creation failed: {}", e))?,
    )
    .filter(LowerCaser)
    .filter(RemoveLongFilter::limit(64))
    .build();
    index.tokenizers().register(TOKENIZER_NAME, tokenizer);
    Ok(())
}

fn fields(schema: &Schema) -> Result<TantivyFields, String> {
    let get = |name: &str| {
        schema
            .get_field(name)
            .map_err(|_| format!("Tantivy schema missing {}", name))
    };
    Ok(TantivyFields {
        download_id: get("download_id")?,
        source_exact: get("source_exact")?,
        source_id_exact: get("source_id_exact")?,
        source_id_lower: get("source_id_lower")?,
        source_id_ngram: get("source_id_ngram")?,
        source_id_reading_kana: get("source_id_reading_kana")?,
        source_id_reading_romaji: get("source_id_reading_romaji")?,
        source_url_exact: get("source_url_exact")?,
        source_url_lower: get("source_url_lower")?,
        source_url_ngram: get("source_url_ngram")?,
        source_url_reading_kana: get("source_url_reading_kana")?,
        source_url_reading_romaji: get("source_url_reading_romaji")?,
        title_exact: get("title_exact")?,
        title_lower: get("title_lower")?,
        title_ngram: get("title_ngram")?,
        title_reading_kana: get("title_reading_kana")?,
        title_reading_romaji: get("title_reading_romaji")?,
        author_name_exact: get("author_name_exact")?,
        author_name_lower: get("author_name_lower")?,
        author_name_ngram: get("author_name_ngram")?,
        author_name_reading_kana: get("author_name_reading_kana")?,
        author_name_reading_romaji: get("author_name_reading_romaji")?,
        author_id_exact: get("author_id_exact")?,
        author_id_lower: get("author_id_lower")?,
        author_id_ngram: get("author_id_ngram")?,
        author_id_reading_kana: get("author_id_reading_kana")?,
        author_id_reading_romaji: get("author_id_reading_romaji")?,
        tags_exact: get("tags_exact")?,
        tags_lower: get("tags_lower")?,
        tags_ngram: get("tags_ngram")?,
        tags_reading_kana: get("tags_reading_kana")?,
        tags_reading_romaji: get("tags_reading_romaji")?,
        series_title_exact: get("series_title_exact")?,
        series_title_lower: get("series_title_lower")?,
        series_title_ngram: get("series_title_ngram")?,
        series_title_reading_kana: get("series_title_reading_kana")?,
        series_title_reading_romaji: get("series_title_reading_romaji")?,
        excerpt_exact: get("excerpt_exact")?,
        excerpt_lower: get("excerpt_lower")?,
        excerpt_ngram: get("excerpt_ngram")?,
        excerpt_reading_kana: get("excerpt_reading_kana")?,
        excerpt_reading_romaji: get("excerpt_reading_romaji")?,
        body_exact: get("body_exact")?,
        body_lower: get("body_lower")?,
        body_ngram: get("body_ngram")?,
        body_reading_kana: get("body_reading_kana")?,
        body_reading_romaji: get("body_reading_romaji")?,
        body_chunk: get("body_chunk")?,
        published_at: get("published_at")?,
        downloaded_at: get("downloaded_at")?,
        favorite: get("favorite")?,
        watch_updates: get("watch_updates")?,
        asset_kinds: get("asset_kinds")?,
        text_length: get("text_length")?,
    })
}

fn add_search_text(
    doc: &mut TantivyDocument,
    exact: Field,
    lower: Field,
    ngram: Field,
    reading_kana: Field,
    reading_romaji: Field,
    value: &str,
) {
    doc.add_text(exact, value);
    doc.add_text(lower, normalize_for_search(value));
    doc.add_text(ngram, search_index_text(value));
    doc.add_text(reading_kana, reading_kana_text(value));
    doc.add_text(reading_romaji, reading_romaji_text(value));
}

fn text_value(doc: &TantivyDocument, field: Field) -> String {
    doc.get_first(field)
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string()
}

fn escape_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|part| format!("\"{}\"", part.replace('"', "\\\"")))
        .collect::<Vec<String>>()
        .join(" ")
}
