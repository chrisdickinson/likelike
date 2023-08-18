use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use serde::{Serialize, Deserialize};
use slugify::slugify;
use std::collections::HashMap;
use std::fs::read_to_string;
use std::str::FromStr;
use std::{borrow::Cow, fmt::Debug, path::Path};
use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug)]
pub struct LinkSource<'a> {
    pub(crate) filename: Option<Cow<'a, str>>,
    pub(crate) created: Option<DateTime<Utc>>,
    pub(crate) modified: Option<DateTime<Utc>>,
    pub(crate) content: Cow<'a, str>,
}

impl<'a> LinkSource<'a> {
    pub fn new(
        filename: Option<&'a Path>,
        created: Option<DateTime<Utc>>,
        modified: Option<DateTime<Utc>>,
        content: Cow<'a, str>,
    ) -> Self {
        Self {
            filename: filename.map(|xs| xs.to_string_lossy()),
            created,
            modified,
            content,
        }
    }

    pub fn from_path<'b: 'a>(p: &'b Path) -> eyre::Result<Self> {
        // Example input: "20220115-link-dump.md"
        let mut created = None;
        let mut modified = None;

        'created_from_filename: {
            let Some(filename) = p.file_name() else { break 'created_from_filename };
            let Some(filename) = filename.to_str() else { break 'created_from_filename };
            let Some(maybe_date) = filename.split('-').next() else { break 'created_from_filename };
            let Ok(date) = NaiveDate::parse_from_str(maybe_date, "%Y%m%d") else { break 'created_from_filename };
            let Some(datetime) = date.and_hms_milli_opt(0, 0, 0, 0) else { break 'created_from_filename };
            let Some(datetime) = Local.from_local_datetime(&datetime).latest() else { break 'created_from_filename };

            created.replace(DateTime::<Utc>::from(datetime));
        }

        'modified_from_fs: {
            let Ok(metadata) = std::fs::metadata(p) else { break 'modified_from_fs };
            let Ok(mtime) = metadata.modified() else { break 'modified_from_fs };

            modified.replace(DateTime::<Utc>::from(mtime));
        }

        let content: Cow<'_, str> = Cow::Owned(read_to_string(p)?);

        Ok(Self {
            filename: Some(p.to_string_lossy()),
            content,
            created,
            modified,
        })
    }

    pub fn filename_string(&self) -> Option<String> {
        self.filename.as_ref().map(|xs| xs.to_string())
    }
}

impl<'inner, 'outer: 'inner> From<&'outer str> for LinkSource<'inner> {
    fn from(xs: &'outer str) -> Self {
        LinkSource::new(None, Some(Utc::now()), Some(Utc::now()), Cow::Borrowed(xs))
    }
}

/// A structure representing metadata about a link from a link dump file.
///
/// Links are uniquely identified by their URL.
///
/// This structure supports tagging, annotating notes on a link, marking "found at",
/// "reaad at", and "published at" data, and surfacing provenance.
#[derive(Default, Clone, Debug)]
pub struct Link {
    pub(crate) url: String,
    pub(crate) title: Option<String>,
    pub(crate) via: Option<Via>,
    pub(crate) tags: Vec<String>,
    pub(crate) notes: Option<String>,

    pub(crate) found_at: Option<DateTime<Utc>>,

    pub(crate) read_at: Option<DateTime<Utc>>,

    pub(crate) published_at: Option<DateTime<Utc>>,

    pub(crate) from_filename: Option<String>,
    pub(crate) image: Option<String>,

    pub(crate) meta: Option<HashMap<String, Vec<String>>>,
    pub(crate) src: Option<Vec<u8>>,
    pub(crate) extracted_text: Option<String>,

    pub(crate) last_fetched: Option<DateTime<Utc>>,

    pub(crate) last_processed: Option<DateTime<Utc>>,

    pub(crate) http_headers: Option<HashMap<String, Vec<String>>>,
}

impl Link {
    pub fn new<T: AsRef<str>, S: AsRef<str>>(url: T, title: S) -> Self {
        Self {
            title: Some(title.as_ref().to_string()),
            url: url.as_ref().to_string(),
            ..Default::default()
        }
    }

    pub fn slug(&self) -> String {
        let title = self
            .meta()
            .and_then(|m| m.get("og:title"))
            .or_else(|| self.meta().and_then(|m| m.get("twitter:title")))
            .and_then(|v| v.get(0))
            .map(|s| s.as_str())
            .or_else(|| self.title())
            .unwrap_or_else(|| self.url());

        slug::slugify(title)
    }

    pub fn is_pdf(&self) -> bool {
        matches!(
            self.http_headers
                .as_ref()
                .and_then(|hdrs| hdrs.get("content-type"))
                .and_then(|xs| xs.last())
                .map(|xs| xs.as_str()),
            Some("application/pdf")
        )
    }

    pub fn is_html(&self) -> bool {
        matches!(
            self.http_headers
                .as_ref()
                .and_then(|hdrs| hdrs.get("content-type"))
                .and_then(|xs| xs.last())
                .map(|xs| xs.as_str()),
            Some(
                "text/html"
                    | "text/html;charset=utf-8"
                    | "text/html;charset=UTF-8"
                    | "text/html; charset=utf-8"
                    | "text/html; charset=UTF-8"
            )
        )
    }

    pub fn is_plaintext(&self) -> bool {
        matches!(
            self.http_headers
                .as_ref()
                .and_then(|hdrs| hdrs.get("content-type"))
                .and_then(|xs| xs.last())
                .map(|xs| xs.as_str()),
            Some(
                "text/plain"
                    | "text/plain;charset=utf-8"
                    | "text/plain;charset=UTF-8"
                    | "text/plain; charset=utf-8"
                    | "text/plain; charset=UTF-8"
            )
        )
    }

    pub fn via_mut(&mut self) -> &mut Option<Via> {
        &mut self.via
    }

    pub fn tags_mut(&mut self) -> &mut Vec<String> {
        &mut self.tags
    }

    pub fn notes_mut(&mut self) -> &mut Option<String> {
        &mut self.notes
    }

    pub fn found_at_mut(&mut self) -> &mut Option<DateTime<Utc>> {
        &mut self.found_at
    }

    pub fn read_at_mut(&mut self) -> &mut Option<DateTime<Utc>> {
        &mut self.read_at
    }

    pub fn published_at_mut(&mut self) -> &mut Option<DateTime<Utc>> {
        &mut self.published_at
    }

    pub fn url(&self) -> &str {
        self.url.as_ref()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn via(&self) -> Option<&Via> {
        self.via.as_ref()
    }

    pub fn tags(&self) -> &Vec<String> {
        &self.tags
    }

    pub fn notes(&self) -> Option<&str> {
        self.notes.as_deref()
    }

    pub fn found_at(&self) -> Option<DateTime<Utc>> {
        self.found_at
    }

    pub fn read_at(&self) -> Option<DateTime<Utc>> {
        self.read_at
    }

    pub fn published_at(&self) -> Option<DateTime<Utc>> {
        self.published_at
    }

    pub fn from_filename(&self) -> Option<&str> {
        self.from_filename.as_deref()
    }

    pub fn image(&self) -> Option<&str> {
        self.image.as_deref()
    }

    pub fn image_mut(&mut self) -> &mut Option<String> {
        &mut self.image
    }

    pub fn src(&self) -> Option<&[u8]> {
        self.src.as_deref()
    }

    pub fn extract_text(&self) -> Option<&str> {
        self.extracted_text.as_deref()
    }

    pub fn last_fetched(&self) -> Option<DateTime<Utc>> {
        self.last_fetched
    }

    pub fn last_fetched_mut(&mut self) -> &mut Option<DateTime<Utc>> {
        &mut self.last_fetched
    }

    pub fn last_processed(&self) -> Option<DateTime<Utc>> {
        self.last_processed
    }

    pub fn last_processed_mut(&mut self) -> &mut Option<DateTime<Utc>> {
        &mut self.last_processed
    }

    pub fn http_headers(&self) -> Option<&HashMap<String, Vec<String>>> {
        self.http_headers.as_ref()
    }

    pub fn meta(&self) -> Option<&HashMap<String, Vec<String>>> {
        self.meta.as_ref()
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum Via {
    Friend(String),
    Link(String),
    Freeform(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Frontmatter {
    title: String,
    slug: String,
    date: String,
    taxonomies: HashMap<String, Vec<String>>,
    extra: FrontmatterExtra,

    #[serde(skip)]
    notes: String,
}

static TOML_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^[[:space:]]*\+\+\+(\r?\n(?s).*?(?-s))\+\+\+[[:space:]]*(?:$|(?:\r?\n((?s).*(?-s))$))",
    )
    .unwrap()
});

impl FromStr for Frontmatter {
    type Err = eyre::Report;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let caps = TOML_RE.captures(s).ok_or_else(|| eyre::eyre!(""))?;
        let front_matter = caps.get(1).ok_or_else(|| eyre::eyre!(""))?.as_str();
        let notes = caps.get(2).map_or("", |m| m.as_str());

        let mut frontmatter: Frontmatter = toml::from_str(front_matter)?;

        frontmatter.notes = notes.to_string();

        Ok(frontmatter)
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(tag = "type", content = "content")]
pub enum FrontmatterVia {
    Friend(String),
    Link(String),
    Freeform(String),
}

impl From<Via> for FrontmatterVia {
    fn from(v: Via) -> Self {
        match v {
            Via::Friend(xs) => FrontmatterVia::Friend(xs),
            Via::Link(xs) => FrontmatterVia::Link(xs),
            Via::Freeform(xs) => FrontmatterVia::Freeform(xs),
        }
    }
}

impl From<FrontmatterVia> for Via {
    fn from(v: FrontmatterVia) -> Self {
        match v {
            FrontmatterVia::Friend(xs) => Via::Friend(xs),
            FrontmatterVia::Link(xs) => Via::Link(xs),
            FrontmatterVia::Freeform(xs) => Via::Freeform(xs),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FrontmatterUrl {
    url: String,
    host: String,
    path_segments: Vec<String>,
    path: String,
    query: HashMap<String, String>,
}

impl From<url::Url> for FrontmatterUrl {
    fn from(u: url::Url) -> Self {
        FrontmatterUrl {
            url: u.to_string(),
            host: u
                .host_str()
                .map(|xs| xs.to_string())
                .unwrap_or_else(Default::default),
            path: u.path().to_string(),
            path_segments: u
                .path_segments()
                .map(|xs| xs.map(|xs| xs.to_string()).collect())
                .unwrap_or_else(Default::default),
            query: u
                .query_pairs()
                .map(|(lhs, rhs)| (lhs.to_string(), rhs.to_string()))
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FrontmatterExtra {
    title: Option<String>,

    found_at: Option<String>,
    read_at: Option<String>,
    published_at: Option<String>,
    from_filename: Option<String>,
    image: Option<String>,
    url: FrontmatterUrl,
    via: Option<FrontmatterVia>,

    meta: HashMap<String, String>,
}

impl Frontmatter {
    pub fn filename(&self) -> String {
        format!("{}.md", slugify!(self.extra.url.url.as_str()))
    }

    pub fn notes(&self) -> &str {
        self.notes.as_str()
    }

    pub fn update_link(self, link: &mut Link) {
        let Self {
            title,
            slug,
            date,
            mut taxonomies,
            notes,
            extra: FrontmatterExtra {
                url,
                via,
                found_at,
                read_at,
                published_at,

                meta,

                from_filename,
                image,
                ..
            },
        } = self;

        link.title = if title.trim().is_empty() { None } else { Some(title) };
        link.via = via.map(Into::into);
        link.tags = taxonomies.remove("tags").unwrap_or_else(Vec::new);
        link.notes = if notes.trim().is_empty() { None } else { Some(notes) };

        'found_at: {
            if let Some(found_at) = found_at {
                let Some(found_at) = NaiveDate::parse_from_str(found_at.as_str(), "%Y-%m-%d").ok() else { break 'found_at };
                let Some(found_at) = found_at.and_hms_milli_opt(0, 0, 0, 0) else { break 'found_at };
                let Some(found_at) = found_at.and_local_timezone(Utc).earliest() else { break 'found_at };
                link.found_at = Some(found_at);
            }
        }

        'read_at: {
            if let Some(read_at) = read_at {
                let Some(read_at) = NaiveDate::parse_from_str(read_at.as_str(), "%Y-%m-%d").ok() else { break 'read_at };
                let Some(read_at) = read_at.and_hms_milli_opt(0, 0, 0, 0) else { break 'read_at };
                let Some(read_at) = read_at.and_local_timezone(Utc).earliest() else { break 'read_at };
                link.read_at = Some(read_at);
            }
        }

        'published_at: {
            if let Some(published_at) = published_at {
                let Some(published_at) = NaiveDate::parse_from_str(published_at.as_str(), "%Y-%m-%d").ok() else { break 'published_at };
                let Some(published_at) = published_at.and_hms_milli_opt(0, 0, 0, 0) else { break 'published_at };
                let Some(published_at) = published_at.and_local_timezone(Utc).earliest() else { break 'published_at };
                link.published_at = Some(published_at);
            }
        }
    }
}

impl TryFrom<Link> for Frontmatter {
    type Error = eyre::ErrReport;

    fn try_from(link: Link) -> eyre::Result<Self> {
        let title = link.title().unwrap_or_else(|| link.url()).to_string();
        let slug = slugify!(link.title().unwrap_or_else(|| link.url()));
        let date = link
            .published_at()
            .or_else(|| link.found_at())
            .unwrap_or_else(Utc::now);

        let date = date.format("%Y-%m-%d").to_string();
        let mut taxonomies = HashMap::new();

        // This is a little redundant since we do this on import now, but older link entries might contain
        // empty strings in their tags.
        let tags = link
            .tags()
            .iter()
            .filter(|xs| !xs.is_empty())
            .map(|xs| xs.to_owned())
            .collect();

        taxonomies.insert("tags".to_string(), tags);

        Ok(Self {
            title,
            slug,
            date,
            taxonomies,
            notes: link.notes.unwrap_or_default(),
            extra: FrontmatterExtra {
                url: link.url.parse::<url::Url>()?.into(),
                title: link.title,
                via: link.via.map(|xs| xs.into()),
                found_at: link.found_at.map(|xs| xs.format("%Y-%m-%d").to_string()),
                read_at: link.read_at.map(|xs| xs.format("%Y-%m-%d").to_string()),
                published_at: link
                    .published_at
                    .map(|xs| xs.format("%Y-%m-%d").to_string()),

                meta: link
                    .meta
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, mut v)| (k, v.pop().unwrap_or_default()))
                    .collect(),

                from_filename: link.from_filename,
                image: link.image,
            },
        })
    }
}
