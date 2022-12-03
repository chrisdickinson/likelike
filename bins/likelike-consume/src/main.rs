use futures::StreamExt;
use std::{borrow::Cow, path::PathBuf};
use tokio::fs::read_to_string;

use clap::Parser;
use likelike::{
    process_input, DummyWrap, HttpClientWrap, LinkSource, ReadLinkInformation, SqliteStore,
};

/// Process markdown-formatted linkdump files and store them in a sqlite database. The database
/// defaults to in-memory.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    files: Vec<PathBuf>,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Args::parse();

    let store = SqliteStore::new().await;
    let store = HttpClientWrap::wrap(store);
    for file in cli.files {
        eprintln!("file={:?}", file);
        let Ok(link_source) = LinkSource::from_path(file.as_path()) else { continue };
        process_input(link_source, &store).await?;
    }

    let mut iter = store.values().await?;
    while let Some(next) = iter.next().await {
        println!(
            "{:18?} {:64} {}",
            next.from_filename(),
            next.title().unwrap_or("idk"),
            next.url()
        );
    }

    Ok(())
}
