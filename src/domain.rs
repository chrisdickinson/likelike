use chrono::serde::ts_seconds_option;
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use slugify::slugify;
use std::collections::HashMap;
use std::fs::read_to_string;
use std::{borrow::Cow, collections::HashSet, fmt::Debug, path::Path};

#[derive(Debug)]
pub struct LinkSource<'a> {
    pub(crate) filename: Option<Cow<'a, str>>,
    pub(crate) created: Option<DateTime<Utc>>,
    pub(crate) content: Cow<'a, str>,
}

impl<'a> LinkSource<'a> {
    pub fn new(
        filename: Option<&'a Path>,
        created: Option<DateTime<Utc>>,
        content: Cow<'a, str>,
    ) -> Self {
        Self {
            filename: filename.map(|xs| xs.to_string_lossy()),
            created,
            content,
        }
    }

    pub fn from_path<'b: 'a>(p: &'b Path) -> eyre::Result<Self> {
        // Example input: "20220115-link-dump.md"
        let mut created = None;

        'a: {
            let Some(filename) = p.file_name() else { break 'a };
            let Some(filename) = filename.to_str() else { break 'a };
            let Some(maybe_date) = filename.split('-').next() else { break 'a };
            let Ok(date) = NaiveDate::parse_from_str(maybe_date, "%Y%m%d") else { break 'a };
            let Some(datetime) = date.and_hms_milli_opt(0, 0, 0, 0) else { break 'a };
            let Some(datetime) = Local.from_local_datetime(&datetime).latest() else { break 'a };

            created.replace(DateTime::<Utc>::from(datetime));
        }

        let content: Cow<'_, str> = Cow::Owned(read_to_string(p)?);

        Ok(Self {
            filename: Some(p.to_string_lossy()),
            content,
            created,
        })
    }

    pub fn filename_string(&self) -> Option<String> {
        self.filename.as_ref().map(|xs| xs.to_string())
    }
}

impl<'inner, 'outer: 'inner> From<&'outer str> for LinkSource<'inner> {
    fn from(xs: &'outer str) -> Self {
        LinkSource::new(None, Some(Utc::now()), Cow::Borrowed(xs))
    }
}

/// A structure representing metadata about a link from a link dump file.
///
/// Links are uniquely identified by their URL.
///
/// This structure supports tagging, annotating notes on a link, marking "found at",
/// "reaad at", and "published at" data, and surfacing provenance.
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub(crate) url: String,
    pub(crate) title: Option<String>,
    pub(crate) via: Option<Via>,
    pub(crate) tags: HashSet<String>,
    pub(crate) notes: Option<String>,

    #[serde(with = "ts_seconds_option")]
    pub(crate) found_at: Option<DateTime<Utc>>,

    #[serde(with = "ts_seconds_option")]
    pub(crate) read_at: Option<DateTime<Utc>>,

    #[serde(with = "ts_seconds_option")]
    pub(crate) published_at: Option<DateTime<Utc>>,

    pub(crate) from_filename: Option<String>,
    pub(crate) image: Option<String>,
}

impl Link {
    pub fn new<T: AsRef<str>, S: AsRef<str>>(url: T, title: S) -> Self {
        Self {
            title: Some(title.as_ref().to_string()),
            url: url.as_ref().to_string(),
            ..Default::default()
        }
    }

    pub fn via_mut(&mut self) -> &mut Option<Via> {
        &mut self.via
    }

    pub fn tags_mut(&mut self) -> &mut HashSet<String> {
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

    pub fn tags(&self) -> &HashSet<String> {
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
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum Via {
    Friend(String),
    Link(String),
    Freeform(String),
}

#[derive(Serialize)]
pub struct Frontmatter {
    title: String,
    slug: String,
    date: String,
    taxonomies: HashMap<String, Vec<String>>,
    extra: FrontmatterExtra,

    #[serde(skip_serializing)]
    notes: String,
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

#[derive(Serialize)]
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

#[derive(Serialize)]
pub struct FrontmatterExtra {
    title: Option<String>,

    found_at: Option<String>,
    read_at: Option<String>,
    published_at: Option<String>,
    from_filename: Option<String>,
    image: Option<String>,
    url: FrontmatterUrl,
    via: Option<FrontmatterVia>,
}

impl Frontmatter {
    pub fn filename(&self) -> String {
        format!("{}.md", slugify!(self.extra.url.url.as_str()))
    }

    pub fn notes(&self) -> &str {
        self.notes.as_str()
    }
}

impl TryFrom<Link> for Frontmatter {
    type Error = eyre::ErrReport;

    fn try_from(link: Link) -> eyre::Result<Self> {
        let title = format!("Reading: {}", link.title().unwrap_or_else(|| link.url()));
        let slug = slugify!(link.title().unwrap_or_else(|| link.url()));
        let date = link
            .published_at()
            .or_else(|| link.found_at())
            .unwrap_or_else(Utc::now);

        let date = date.format("%Y-%m-%d").to_string();
        let mut taxonomies = HashMap::new();

        taxonomies.insert("tags".to_string(), link.tags().iter().cloned().collect());

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

                from_filename: link.from_filename,
                image: link.image,
            },
        })
    }
}
