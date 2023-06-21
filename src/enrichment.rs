use crate::domain::{Link, LinkSource};
use crate::LinkReader;
use chrono::Utc;

pub(crate) async fn enrich_link<Store>(
    mut link: Link,
    store: &Store,
    link_source: &LinkSource<'_>,
) -> eyre::Result<Link>
where
    Store: LinkReader + Send + Sync,
{
    if let Some(mut known_link) = store.get(link.url.as_str()).await? {
        known_link.read_at = {
            if let Some(notes) = link.notes() {
                if !notes.trim().is_empty() {
                    known_link
                        .read_at
                        .or(link_source.modified)
                        .or_else(|| Some(Utc::now()))
                } else {
                    None
                }
            } else {
                None
            }
        };

        known_link.found_at = known_link
            .found_at
            .or(link.found_at)
            .or(link_source.created);

        known_link.from_filename = known_link
            .from_filename
            .or(link.from_filename)
            .or_else(|| link_source.filename_string());

        known_link.title = link.title.or(known_link.title);
        known_link.notes = link.notes;
        known_link.tags = link.tags;
        known_link.via = link.via;

        link = known_link;
    } else {
        link.found_at = link_source.modified.or(link_source.created);
        link.from_filename = link_source.filename_string();

        if let Some(notes) = link.notes() {
            if !notes.trim().is_empty() {
                link.read_at = link_source.modified.or(link_source.created);
            }
        }
    }

    Ok(link)
}
