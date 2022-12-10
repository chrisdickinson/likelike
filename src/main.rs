use chrono::Utc;
use futures::{future::join_all, pin_mut, Stream, StreamExt};
use serde::Serialize;
use slugify::slugify;
use std::{collections::HashMap, path::PathBuf};

use clap::Parser;
use likelike::{process_input, HttpClientWrap, Link, LinkSource, ReadLinkInformation, SqliteStore};

/// Process markdown-formatted linkdump files and store them in a sqlite database. The database
/// defaults to in-memory.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long)]
    database_url: Option<String>,
}

#[derive(Parser, Debug)]
enum Commands {
    Import { files: Vec<PathBuf> },
    Export { output: PathBuf },
}

#[derive(Serialize)]
struct Frontmatter {
    title: String,
    slug: String,
    date: String,
    taxonomies: HashMap<String, Vec<String>>,
    extra: Link,
}

impl From<Link> for Frontmatter {
    fn from(link: Link) -> Self {
        let title = format!("Reading: {}", link.title().unwrap_or_else(|| link.url()));
        let slug = slugify!(link.title().unwrap_or_else(|| link.url()));
        let date = link
            .published_at()
            .or(link.found_at())
            .unwrap_or_else(|| Utc::now());

        let date = date.format("%Y-%m-%d").to_string();
        let mut taxonomies = HashMap::new();

        taxonomies.insert(
            "tags".to_string(),
            link.tags().into_iter().cloned().collect(),
        );

        Self {
            title,
            slug,
            date,
            taxonomies,
            extra: link,
        }
    }
}

impl Frontmatter {
    fn filename(&self) -> String {
        format!("{}.md", slugify!(self.extra.url()))
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Args::parse();

    let store = if let Some(db_url) = cli.database_url {
        SqliteStore::with_connection_string(db_url).await?
    } else {
        SqliteStore::new().await
    };

    match cli.command {
        Commands::Export { output } => {
            let mut links = store.values().await?;

            while let Some(link) = links.next().await {
                if link.read_at().is_none() {
                    continue;
                }

                let frontmatter: Frontmatter = link.into();
                let mut path = output.clone();
                path.push(frontmatter.filename());
                std::fs::write(
                    path,
                    format!(
                        "+++\n{}\n+++\n{}",
                        toml::to_string_pretty(&frontmatter)?,
                        frontmatter.extra.notes().unwrap_or("")
                    ),
                )?;
            }
        }

        Commands::Import { files } => {
            let store = HttpClientWrap::wrap(store);
            let store = &store;
            let mut futs = Vec::with_capacity(files.len());
            for file in files.into_iter() {
                futs.push(async move {
                    let link_source = LinkSource::from_path(file.as_path())?;
                    process_input(link_source, store).await?;
                    Ok(file) as eyre::Result<PathBuf>
                });
            }

            for result in join_all(futs.into_iter()).await {
                let Ok(file) = result else { continue };
                eprintln!("processed \"{}\"", file.to_string_lossy());
            }
        }
    }

    Ok(())
}