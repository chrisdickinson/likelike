#![allow(dead_code)]
#![allow(unused_variables)]

use async_stream::stream;
use chrono::{TimeZone, Utc};
use futures::{stream, Stream};
use include_dir::{include_dir, Dir};
use reqwest::{header::HeaderMap, redirect::Policy, Client, ClientBuilder};
use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions, Row, SqliteConnection};
use std::{collections::HashMap, env, fmt::Debug, pin::Pin, str::FromStr, time::Duration};
use tokio::sync::Mutex;

use crate::{FetchLinkMetadata, Link, ReadLinkInformation, WriteLinkInformation};

pub struct HttpClientWrap<T> {
    client: Client,
    inner: T,
}

impl<T> HttpClientWrap<T> {
    pub fn new(client: Client, inner: T) -> Self {
        Self { client, inner }
    }

    pub fn wrap(inner: T) -> Self {
        let agent = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"), " (github.com/chrisdickinson/likelike)");

        let max_redirects: usize = std::env::var("LIKELIKE_MAX_REDIRECTS")
            .ok()
            .and_then(|xs| xs.parse().ok())
            .unwrap_or(10);

        let timeout: u64 = std::env::var("LIKELIKE_REQUEST_TIMEOUT_SECONDS")
            .ok()
            .and_then(|xs| xs.parse().ok())
            .unwrap_or(15);

        let client = ClientBuilder::new()
            .redirect(Policy::limited(max_redirects))
            .user_agent(agent)
            .timeout(Duration::new(timeout, 0))
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .build()
            .expect("default reqwest client could not be constructed");

        Self { inner, client }
    }
}

#[async_trait::async_trait]
impl<T: Send + Sync> FetchLinkMetadata for HttpClientWrap<T> {
    type Headers = HeaderMap;
    type Body = Pin<Box<dyn Stream<Item = bytes::Bytes>>>;

    async fn fetch(&self, link: &Link) -> eyre::Result<Option<(Self::Headers, Self::Body)>> {
        let response = match self.client.get(link.url()).send().await {
            Ok(response) => response,
            Err(e) => {
                // We don't know if there's _really_ a problem if we can't connect: it could be our
                // local network or the site could be temporarily unavailable. We only really want
                // to throw up our hands if we're getting "oh no this site is complete garbage!"
                if e.is_connect() {
                    return Ok(None);
                } else {
                    return Err(e.into());
                }
            }
        };

        if !response.status().is_success() {
            return Ok(None);
        }

        let headers = response.headers().clone();
        let body = response.bytes_stream();

        let stream = stream! {
            for await value in body {
                let Ok(value) = value else { continue };
                yield value
            }
        };

        Ok(Some((headers, Box::pin(stream))))
    }
}

#[async_trait::async_trait]
impl<T: ReadLinkInformation + Send + Sync> ReadLinkInformation for HttpClientWrap<T> {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        self.inner.get(link).await
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        self.inner.values().await
    }
}

#[async_trait::async_trait]
impl<T: WriteLinkInformation + Send + Sync> WriteLinkInformation for HttpClientWrap<T> {
    async fn write(&self, link: &Link) -> eyre::Result<bool> {
        self.inner.write(link).await
    }
}

pub struct DummyWrap<T> {
    inner: T,
}

impl<T> DummyWrap<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl<T: Send + Sync> FetchLinkMetadata for DummyWrap<T> {
    type Headers = HeaderMap;
    type Body = Pin<Box<dyn Stream<Item = bytes::Bytes>>>;

    async fn fetch(&self, link: &Link) -> eyre::Result<Option<(Self::Headers, Self::Body)>> {
        Ok(None)
    }
}

#[async_trait::async_trait]
impl<T: ReadLinkInformation + Send + Sync> ReadLinkInformation for DummyWrap<T> {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        self.inner.get(link).await
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        self.inner.values().await
    }
}

#[async_trait::async_trait]
impl<T: WriteLinkInformation + Send + Sync> WriteLinkInformation for DummyWrap<T> {
    async fn write(&self, link: &Link) -> eyre::Result<bool> {
        self.inner.write(link).await
    }
}

/// An in-memory link store.
#[derive(Default, Debug)]
struct InMemoryStore {
    data: Mutex<HashMap<String, Link>>,
}

impl InMemoryStore {
    fn new() -> Self {
        Self {
            ..Default::default()
        }
    }
}

#[async_trait::async_trait]
impl ReadLinkInformation for InMemoryStore {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        let data = self.data.lock().await;

        Ok(data.get(link).cloned())
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let data = self.data.lock().await;

        // This "collect()" seems to be doing something for us, since implementing clippy's suggestion
        // nets us an E0597 lifetime error.
        #[allow(clippy::needless_collect)]
        let values: Vec<_> = data.values().cloned().collect();

        Ok(Box::pin(stream::iter(values.into_iter())))
    }
}

#[async_trait::async_trait]
impl WriteLinkInformation for InMemoryStore {
    async fn write(&self, link: &Link) -> eyre::Result<bool> {
        let mut data = self.data.lock().await;
        data.insert(link.url.clone(), link.clone());
        Ok(true)
    }
}

static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

#[derive(Debug)]
pub struct SqliteStore {
    sqlite: Mutex<SqliteConnection>,
}

impl SqliteStore {
    pub async fn new() -> Self {
        let env_var = env::var("LIKELIKE_DB");

        Self::with_connection_string(if let Ok(db_url) = env_var {
            db_url
        } else {
            let location = dirs::data_local_dir()
                .map(|mut xs| {
                    std::fs::create_dir_all(&xs).expect("Must be able to create XDG_SHARE_HOME");
                    xs.push("likelike.sqlite3");
                    xs.to_string_lossy().to_string()
                })
                .unwrap_or_else(|| ":memory:".to_string());

            format!("sqlite://{}", location)
        })
        .await
        .unwrap()
    }

    pub async fn with_connection_string(s: impl AsRef<str>) -> eyre::Result<Self> {
        Self::with_connection_options(SqliteConnectOptions::from_str(s.as_ref())?).await
    }

    pub async fn with_connection_options(opts: SqliteConnectOptions) -> eyre::Result<Self> {
        let mut sqlite = opts.create_if_missing(true).connect().await?;

        let mut files: Vec<_> = MIGRATIONS_DIR.files().collect();
        files.sort_by(|lhs, rhs| lhs.path().cmp(rhs.path()));
        let version = files.len();

        let last_index = sqlx::query("select version from database_version limit 1")
            .fetch_one(&mut sqlite)
            .await
            .map(|result| result.get("version"))
            .unwrap_or_else(|_| 0u32);

        let migration_count = files.len() as u32;
        for file in files.into_iter().skip(last_index as usize) {
            sqlx::query(unsafe { std::str::from_utf8_unchecked(file.contents()) })
                .execute(&mut sqlite)
                .await?;
        }

        sqlx::query(r#"
            insert into database_version (id, version) values (0, ?) on conflict(id) do update set version = excluded.version
        "#).bind(migration_count).execute(&mut sqlite)
            .await?;

        Ok(Self {
            sqlite: Mutex::new(sqlite),
        })
    }
}

#[async_trait::async_trait]
impl WriteLinkInformation for SqliteStore {
    async fn write(&self, link: &Link) -> eyre::Result<bool> {
        let mut sqlite = self.sqlite.lock().await;
        let tags = serde_json::to_string(&link.tags)?;
        let via = serde_json::to_string(&link.via)?;

        let found_at = link.found_at.map(|xs| xs.timestamp_millis());
        let read_at = link.read_at.map(|xs| xs.timestamp_millis());

        let last_fetched = link.last_fetched.map(|xs| xs.timestamp_millis());
        let last_processed = link.last_processed.map(|xs| xs.timestamp_millis());

        let src = link
            .src
            .iter()
            .filter_map(|src| zstd::encode_all(src.as_slice(), 0).ok())
            .next();
        let meta = link
            .meta
            .iter()
            .filter_map(|src| serde_json::to_string(src).ok())
            .next();
        let http_headers = link
            .http_headers
            .iter()
            .filter_map(|http_headers| serde_json::to_vec(http_headers).ok())
            .filter_map(|src| zstd::encode_all(src.as_slice(), 0).ok())
            .next();

        let results = sqlx::query!(
            r#"
            INSERT INTO "links" (
                title,
                tags,
                via,
                notes,
                found_at,
                read_at,
                from_filename,
                url,
                image,
                src,
                meta,
                last_fetched,
                last_processed,
                http_headers
            ) VALUES (
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?
            ) ON CONFLICT (url) DO UPDATE
                SET title=excluded.title,
                    tags=excluded.tags,
                    via=excluded.via,
                    notes=excluded.notes,
                    found_at=excluded.found_at,
                    read_at=excluded.read_at,
                    from_filename=excluded.from_filename,
                    image=excluded.image,
                    src=excluded.src,
                    meta=excluded.meta,
                    last_fetched=excluded.last_fetched,
                    last_processed=excluded.last_processed,
                    http_headers=excluded.http_headers
            "#,
            link.title,
            tags,
            via,
            link.notes,
            found_at,
            read_at,
            link.from_filename,
            link.url,
            link.image,
            src,
            meta,
            last_fetched,
            last_processed,
            http_headers
        )
        .execute(&mut *sqlite)
        .await?;

        Ok(results.rows_affected() > 0)
    }
}

struct LinkRow {
    url: String,
    title: Option<String>,
    tags: String,
    via: Option<String>,
    notes: Option<String>,
    found_at: Option<i64>,
    read_at: Option<i64>,
    published_at: Option<i64>,
    from_filename: Option<String>,
    image: Option<String>,
    src: Option<Vec<u8>>,
    meta: Option<String>,
    last_fetched: Option<i64>,
    last_processed: Option<i64>,
    http_headers: Option<Vec<u8>>,
}

impl TryFrom<LinkRow> for Link {
    type Error = eyre::Error;

    fn try_from(value: LinkRow) -> Result<Self, Self::Error> {
        let found_at = value
            .found_at
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let read_at = value
            .read_at
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let published_at = value
            .published_at
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let last_fetched = value
            .last_fetched
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let last_processed = value
            .last_processed
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let meta = value
            .meta
            .iter()
            .find_map(|src| serde_json::from_str(src).ok());

        let src = value
            .src
            .iter()
            .find_map(|src| zstd::decode_all(src.as_slice()).ok())
            .map(Into::into);

        let http_headers = value
            .http_headers
            .iter()
            .filter_map(|src| zstd::decode_all(src.as_slice()).ok())
            .find_map(|src| serde_json::from_slice(src.as_slice()).ok());

        Ok(Link {
            url: value.url,
            title: value.title,
            tags: serde_json::from_str(&value.tags[..])?,
            via: value
                .via
                .and_then(|via| serde_json::from_str(&via[..]).ok()),
            notes: value.notes,
            found_at,
            read_at,
            published_at,
            from_filename: value.from_filename,
            image: value.image,
            src,
            meta,
            last_fetched,
            last_processed,
            http_headers,
        })
    }
}

#[async_trait::async_trait]
impl ReadLinkInformation for SqliteStore {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        let mut sqlite = self.sqlite.lock().await;
        let Some(value) = sqlx::query_as!(
            LinkRow,
            r#"
            SELECT
                url,
                title,
                tags,
                via,
                notes,
                found_at,
                read_at,
                published_at,
                from_filename,
                image,
                src,
                meta,
                last_fetched,
                last_processed,
                http_headers
            FROM "links" WHERE "url" = ?"#,
            link
        )
        .fetch_optional(&mut *sqlite)
        .await? else { return Ok(None) };

        Ok(Some(value.try_into()?))
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let mut sqlite = self.sqlite.lock().await;

        let stream = stream! {
            let input = sqlx::query_as!(
                LinkRow,
                r#"
                SELECT
                    url,
                    title,
                    tags,
                    via,
                    notes,
                    found_at,
                    read_at,
                    published_at,
                    from_filename,
                    image,
                    NULL as "src?: Vec<u8>", -- explicitly DO NOT FETCH the source data
                    meta,
                    last_fetched,
                    last_processed,
                    http_headers
                FROM "links"
                "#,
            )
            .fetch(&mut *sqlite);

            for await value in input {
                let Ok(value) = value else { continue };
                let Ok(link) = value.try_into() else { continue };

                yield link
            }
        };

        Ok(Box::pin(stream))
    }

    async fn glob<'a, 'b: 'a>(&'a self, pattern: &'b str) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let mut sqlite = self.sqlite.lock().await;

        let stream = stream! {
            let input = sqlx::query_as!(
                LinkRow,
                r#"
                SELECT
                    url,
                    title,
                    tags,
                    via,
                    notes,
                    found_at,
                    read_at,
                    published_at,
                    from_filename,
                    image,
                    NULL as "src?: Vec<u8>", -- explicitly DO NOT FETCH the source data
                    meta,
                    last_fetched,
                    last_processed,
                    http_headers
                FROM "links"
                WHERE url GLOB ?
                "#,
                pattern
            )
            .fetch(&mut *sqlite);

            for await value in input {
                let Ok(value) = value else { continue };
                let Ok(link) = value.try_into() else { continue };

                yield link
            }
        };

        Ok(Box::pin(stream))
    }
}
