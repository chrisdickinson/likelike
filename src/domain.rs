use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
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

use chrono::serde::ts_seconds_option;

/// A structure representing metadata about a link from a link dump file.
///
/// Links are uniquely identified by their URL.
///
/// This structure supports tagging, annotating notes on a link, marking "found at",
/// "reaad at", and "published at" data, and surfacing provenance.
#[allow(dead_code)]
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

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
pub enum Via {
    Friend(String),
    Link(String),
    Freeform(String),
    #[default]
    Unknown,
}
