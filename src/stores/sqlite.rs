use async_stream::stream;
use chrono::{TimeZone, Utc};
use futures::Stream;
use include_dir::{include_dir, Dir};

use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions, Row, SqliteConnection};
use std::{env, fmt::Debug, pin::Pin, str::FromStr};
use tokio::sync::Mutex;

use crate::{Link, LinkReader, LinkWriter};

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
                    xs.push("likelike");
                    std::fs::create_dir_all(&xs).expect("Must be able to create XDG_SHARE_HOME");
                    xs.push("db.sqlite3");
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

        if migration_count != last_index {
            sqlx::query(r#"
                insert into database_version (id, version) values (0, ?) on conflict(id) do update set version = excluded.version
            "#).bind(migration_count).execute(&mut sqlite)
                .await?;
        }

        Ok(Self {
            sqlite: Mutex::new(sqlite),
        })
    }
}

/// Parameters for paginated link listing.
pub struct ListParams {
    pub query: Option<String>,
    pub tag: Option<String>,
    pub hidden: Option<bool>,
    pub offset: i64,
    pub limit: i64,
}

impl SqliteStore {
    /// Counts links matching the given filters.
    pub async fn count(&self, params: &ListParams) -> eyre::Result<i64> {
        let mut sqlite = self.sqlite.lock().await;
        let mut sql = String::from(r#"SELECT COUNT(*) as cnt FROM "links" WHERE 1=1"#);
        if params.query.is_some() {
            sql.push_str(r#" AND (url LIKE '%' || ? || '%' OR title LIKE '%' || ? || '%')"#);
        }
        if params.tag.is_some() {
            sql.push_str(r#" AND tags LIKE '%' || ? || '%'"#);
        }
        if params.hidden.is_some() {
            sql.push_str(" AND hidden = ?");
        }

        let mut q = sqlx::query_scalar::<_, i32>(&sql);
        if let Some(ref query) = params.query {
            q = q.bind(query).bind(query);
        }
        if let Some(ref tag) = params.tag {
            q = q.bind(tag);
        }
        if let Some(hidden) = params.hidden {
            q = q.bind(if hidden { 1i64 } else { 0i64 });
        }

        let count = q.fetch_one(&mut *sqlite).await?;
        Ok(count as i64)
    }

    /// Lists links with pagination and optional filters.
    pub async fn list(&self, params: &ListParams) -> eyre::Result<Vec<Link>> {
        let mut sqlite = self.sqlite.lock().await;
        let mut sql = String::from(
            r#"SELECT url, title, tags, via, notes, found_at, read_at, published_at,
               from_filename, image, meta, last_fetched, last_processed, hidden
               FROM "links" WHERE 1=1"#,
        );
        if params.query.is_some() {
            sql.push_str(r#" AND (url LIKE '%' || ? || '%' OR title LIKE '%' || ? || '%')"#);
        }
        if params.tag.is_some() {
            sql.push_str(r#" AND tags LIKE '%' || ? || '%'"#);
        }
        if params.hidden.is_some() {
            sql.push_str(" AND hidden = ?");
        }
        sql.push_str(" ORDER BY found_at DESC LIMIT ? OFFSET ?");

        let mut q = sqlx::query(&sql);
        if let Some(ref query) = params.query {
            q = q.bind(query).bind(query);
        }
        if let Some(ref tag) = params.tag {
            q = q.bind(tag);
        }
        if let Some(hidden) = params.hidden {
            q = q.bind(if hidden { 1i64 } else { 0i64 });
        }
        q = q.bind(params.limit).bind(params.offset);

        let rows = q.fetch_all(&mut *sqlite).await?;
        let mut links = Vec::with_capacity(rows.len());
        for row in rows {
            let link = Link {
                url: row.get("url"),
                title: row.get("title"),
                tags: row
                    .get::<Option<String>, _>("tags")
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or_default(),
                via: row
                    .get::<Option<String>, _>("via")
                    .and_then(|v| serde_json::from_str(&v).ok()),
                notes: row.get("notes"),
                found_at: row
                    .get::<Option<i64>, _>("found_at")
                    .and_then(|ts| Utc.timestamp_millis_opt(ts).latest()),
                read_at: row
                    .get::<Option<i64>, _>("read_at")
                    .and_then(|ts| Utc.timestamp_millis_opt(ts).latest()),
                published_at: row
                    .get::<Option<i64>, _>("published_at")
                    .and_then(|ts| Utc.timestamp_millis_opt(ts).latest()),
                from_filename: row.get("from_filename"),
                image: row.get("image"),
                meta: row
                    .get::<Option<String>, _>("meta")
                    .and_then(|m| serde_json::from_str(&m).ok()),
                last_fetched: row
                    .get::<Option<i64>, _>("last_fetched")
                    .and_then(|ts| Utc.timestamp_millis_opt(ts).latest()),
                last_processed: row
                    .get::<Option<i64>, _>("last_processed")
                    .and_then(|ts| Utc.timestamp_millis_opt(ts).latest()),
                hidden: row.get::<Option<i64>, _>("hidden").unwrap_or(0) != 0,
                ..Default::default()
            };
            links.push(link);
        }
        Ok(links)
    }

    /// Returns all distinct tags.
    pub async fn all_tags(&self) -> eyre::Result<Vec<String>> {
        let mut sqlite = self.sqlite.lock().await;
        let rows = sqlx::query(r#"SELECT tags FROM "links""#)
            .fetch_all(&mut *sqlite)
            .await?;

        let mut tags = std::collections::BTreeSet::new();
        for row in rows {
            let raw: String = row.get("tags");
            if let Ok(parsed) = serde_json::from_str::<Vec<String>>(&raw) {
                for tag in parsed {
                    if !tag.is_empty() {
                        tags.insert(tag);
                    }
                }
            }
        }
        Ok(tags.into_iter().collect())
    }
}

#[async_trait::async_trait]
impl LinkWriter for SqliteStore {
    async fn write(&self, link: Link) -> eyre::Result<bool> {
        let mut sqlite = self.sqlite.lock().await;
        let tags = serde_json::to_string(&link.tags)?;
        let via = serde_json::to_string(&link.via)?;

        let found_at = link.found_at.map(|xs| xs.timestamp_millis());
        let read_at = link.read_at.map(|xs| xs.timestamp_millis());
        let published_at = link.published_at.map(|xs| xs.timestamp_millis());

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

        let hidden = if link.hidden { 1i64 } else { 0i64 };

        let results = sqlx::query!(
            r#"
            INSERT INTO "links" (
                title,
                tags,
                via,
                notes,
                found_at,
                read_at,
                published_at,
                from_filename,
                url,
                image,
                src,
                meta,
                last_fetched,
                last_processed,
                http_headers,
                hidden
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
                    published_at=excluded.published_at,
                    from_filename=excluded.from_filename,
                    image=excluded.image,
                    src=excluded.src,
                    meta=excluded.meta,
                    last_fetched=excluded.last_fetched,
                    last_processed=excluded.last_processed,
                    http_headers=excluded.http_headers,
                    hidden=excluded.hidden
            "#,
            link.title,
            tags,
            via,
            link.notes,
            found_at,
            read_at,
            published_at,
            link.from_filename,
            link.url,
            link.image,
            src,
            meta,
            last_fetched,
            last_processed,
            http_headers,
            hidden
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
    hidden: Option<i64>,
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
            hidden: value.hidden.unwrap_or(0) != 0,
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl LinkReader for SqliteStore {
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
                http_headers,
                hidden
            FROM "links" WHERE "url" = ?"#,
            link
        )
        .fetch_optional(&mut *sqlite)
        .await? else { return Ok(None) };

        Ok(Some(value.try_into()?))
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a + Send>>> {
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
                    http_headers,
                    hidden
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

    async fn glob<'a, 'b: 'a>(
        &'a self,
        pattern: &'b str,
    ) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
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
                    src,
                    meta,
                    last_fetched,
                    last_processed,
                    http_headers,
                    hidden
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
