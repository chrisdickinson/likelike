use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use futures::{pin_mut, StreamExt};
use html5ever::driver::{self, ParseOpts};

use scraper::{Html, Selector};
use std::{collections::HashMap, str::from_utf8};
use tendril::TendrilSink;

use crate::domain::{Link, LinkSource};
use crate::{FetchLinkMetadata, ReadLinkInformation};

fn process_html(mut link: Link, doc: &Html) -> eyre::Result<Link> {
    link.last_processed = Some(Utc::now());

    let mut pubdate: Option<(usize, DateTime<Utc>)> = None;
    let mut title: Option<(usize, String)> = None;
    let mut image: Option<(usize, String)> = None;
    let mut meta = HashMap::new();

    let mut update_pubdate = |weight, pd: &str| {
        let Ok(pd) = NaiveDate::parse_from_str(pd, "%Y-%m-%d") else { return };
        let Some(pd) = pd.and_hms_milli_opt(0, 0, 0, 0) else { return };
        let Some(pd) = Local.from_local_datetime(&pd).latest() else { return };
        let pd = DateTime::<Utc>::from(pd);

        if let Some((current, _)) = pubdate {
            if current < weight {
                pubdate.replace((weight, pd));
            }
        } else {
            pubdate.replace((weight, pd));
        }
    };

    let mut update_image = |weight, candidate: &str| {
        if let Some((current, _)) = image {
            if current < weight {
                image.replace((weight, candidate.to_string()));
            }
        } else {
            image.replace((weight, candidate.to_string()));
        }
    };

    let mut update_title = |weight, candidate: &str| {
        if let Some((current, _)) = title {
            if current < weight {
                title.replace((weight, candidate.to_string()));
            }
        } else {
            title.replace((weight, candidate.to_string()));
        }
    };

    let selector = Selector::parse(
        r#"
        head title,head meta,time
    "#,
    )
    .expect("selector failed to parse");

    for element in doc.select(&selector) {
        let ev = element.value();
        match ev.name() {
            "title" => {
                let text: String = element.text().collect();
                update_title(2, text.as_str());
            }

            "time" => {
                let text: String = element.text().collect();
                if let Some(datetime) = element.value().attr("datetime") {
                    update_pubdate(2, datetime);
                }
            }

            "meta" => {
                let mut name = None;
                let mut content = None;
                for (attrname, attrvalue) in ev.attrs() {
                    match attrname {
                        "name" => name.replace(attrvalue),
                        "content" => content.replace(attrvalue),
                        _ => continue,
                    };

                    if let (Some(ref name), Some(ref content)) = (&name, &content) {
                        meta.entry(name.to_string())
                            .or_insert_with(Vec::new)
                            .push(content.to_string());
                    }

                    match (name, content) {
                        (None, _) => continue,

                        (Some("title"), Some(title)) => {
                            update_title(5, title);
                        }

                        (Some("og:title"), Some(title)) => {
                            update_title(4, title);
                        }

                        (Some("twitter:title"), Some(title)) => {
                            update_title(3, title);
                        }

                        (Some("twitter:text:title"), Some(title)) => {
                            update_title(0, title);
                        }

                        (Some("og:image:url"), Some(image)) => {
                            update_image(5, image);
                        }

                        (Some("og:image"), Some(image)) => {
                            update_image(5, image);
                        }

                        (Some("twitter:image:src"), Some(image)) => {
                            update_image(4, image);
                        }

                        (Some("twitter:image"), Some(image)) => {
                            update_image(4, image);
                        }

                        (Some("date.created"), Some(pubdate)) => {
                            update_pubdate(5, pubdate);
                        }

                        (Some("date"), Some(pubdate)) => {
                            update_pubdate(4, pubdate);
                        }

                        (Some("article:published_time"), Some(pubdate)) => {
                            update_pubdate(3, pubdate);
                        }

                        (Some("DC.Date"), Some(pubdate)) => {
                            update_pubdate(0, pubdate);
                        }

                        (Some(_), _) => continue,
                    }

                    break;
                }
            }

            _ => {
                let mut collected = 0;
                let text: String = element
                    .text()
                    .take_while(|x| {
                        collected += x.len();
                        collected < 512
                    })
                    .collect();

                if let Some(idx) = text.find("ublished") {
                    // eprintln!("div.<published> = {}", &text[idx..].trim());
                }
            }
        }
    }

    link.title = link.title.or_else(|| title.map(|(_, xs)| xs));
    link.published_at = link.published_at.or_else(|| pubdate.map(|(_, xs)| xs));
    link.image = link.image.or_else(|| image.map(|(_, xs)| xs));
    link.meta = link.meta.or(Some(meta));
    Ok(link)
}

async fn stream_html<Store>(
    mut link: Link,
    store: &Store,
    link_source: &LinkSource<'_>,
    body: <Store as FetchLinkMetadata>::Body,
) -> eyre::Result<Link>
where
    Store: FetchLinkMetadata + ReadLinkInformation + Send + Sync,
{
    let mut parser = driver::parse_document(Html::new_document(), ParseOpts::default());

    pin_mut!(body);
    while let Some(chunk) = body.next().await {
        let Ok(chunk) = from_utf8(chunk.as_ref()) else { break };
        parser.process(chunk.into());
    }

    let doc: Html = parser.finish();
    link = process_html(link, &doc)?;
    link.src = Some(doc.root_element().html().into());
    Ok(link)
}

pub(crate) async fn fetch_link<Store>(
    mut link: Link,
    store: &Store,
    link_source: &LinkSource<'_>,
) -> eyre::Result<Link>
where
    Store: FetchLinkMetadata + ReadLinkInformation + Send + Sync,
{
    if link.last_fetched.is_some() {
        eprintln!("not fetching {}", link.url);
        return Ok(link);
    }

    'fetch: {
        link.last_fetched = Some(Utc::now());
        if let Some((headers, body)) = store.fetch(&link).await? {
            let Ok(headers) = headers.try_into() else { break 'fetch };
            let http_headers = headers
                .into_iter()
                .filter_map(|(key, value)| {
                    let key = key?.as_str().to_lowercase();
                    if matches!(
                        key.as_str(),
                        "set-cookie" |
                    "x-xss-protection" |
                    "strict-transport-security" |
                    "content-security-policy" |
                    "x-content-security-policy" |
                    "vary" |
                    "referrer-policy" |
                    "x-referrer-policy" |
                    "x-frame-options" |
                    "x-content-type-options" |
                    "origin-trial" | // youtube
                    "content-security-policy-report-only" | 
                    "p3p" |
                    "permissions-policy" |
                    "report-to"
                    ) {
                        return None;
                    }

                    Some((key, value.to_str().ok()?.to_string()))
                })
                .fold(HashMap::new(), |mut acc, (key, value)| {
                    acc.entry(key).or_insert_with(Vec::new).push(value);
                    acc
                });

            link.http_headers = Some(http_headers);
            if let Some(
                "text/html"
                | "text/html;charset=utf-8"
                | "text/html;charset=UTF-8"
                | "text/html; charset=utf-8"
                | "text/html; charset=UTF-8",
            ) = link
                .http_headers
                .as_ref()
                .and_then(|hdrs| hdrs.get("content-type"))
                .and_then(|xs| xs.last())
                .map(|xs| xs.as_str())
            {
                return stream_html(link, store, link_source, body).await;
            }
        }
    }

    Ok(link)
}

pub(crate) async fn process_link<Store>(
    mut link: Link,
    store: &Store,
    link_source: &LinkSource<'_>,
) -> eyre::Result<Link>
where
    Store: FetchLinkMetadata + ReadLinkInformation + Send + Sync,
{
    if link.last_processed.is_some() {
        eprintln!("not processing {}", link.url);
        return Ok(link);
    }

    if let (true, Some(src)) = (link.is_html(), &link.src) {
        let html = String::from_utf8_lossy(src.as_ref());
        let doc = Html::parse_document(html.as_ref());
        link = process_html(link, &doc)?;
    }

    Ok(link)
}

pub(crate) async fn enrich_link<Store>(
    mut link: Link,
    store: &Store,
    link_source: &LinkSource<'_>,
) -> eyre::Result<Link>
where
    Store: FetchLinkMetadata + ReadLinkInformation + Send + Sync,
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

    eprintln!("enrich {}...", link.url);
    link = fetch_link(link, store, link_source).await?;
    link = process_link(link, store, link_source).await?;

    Ok(link)
}
