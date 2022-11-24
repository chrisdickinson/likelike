use chrono::{DateTime, Utc};
use std::{collections::HashSet, fmt::Debug};

/// A structure representing metadata about a link from a link dump file.
///
/// Links are uniquely identified by their URL.
///
/// This structure supports tagging, annotating notes on a link, marking "found at",
/// "reaad at", and "published at" data, and surfacing provenance.
#[allow(dead_code)]
#[derive(Default, Clone, Debug)]
pub struct Link {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) via: Option<Via>,
    pub(crate) tags: HashSet<String>,
    pub(crate) notes: Option<String>,
    pub(crate) found_at: Option<DateTime<Utc>>,
    pub(crate) read_at: Option<DateTime<Utc>>,
    pub(crate) published_at: Option<DateTime<Utc>>,
}

impl Link {
    pub fn new<T: AsRef<str>, S: AsRef<str>>(url: T, title: S) -> Self {
        Self {
            title: title.as_ref().to_string(),
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

    pub fn title(&self) -> &str {
        self.title.as_ref()
    }

    pub fn via(&self) -> Option<&Via> {
        self.via.as_ref()
    }

    pub fn tags(&self) -> &HashSet<String> {
        &self.tags
    }

    pub fn notes(&self) -> Option<&String> {
        self.notes.as_ref()
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
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
pub enum Via {
    Friend(String),
    Link(String),
    Freeform(String),
    #[default]
    Unknown,
}
