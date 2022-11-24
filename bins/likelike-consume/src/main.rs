use futures::StreamExt;
use std::path::PathBuf;
use tokio::fs::read_to_string;

use clap::Parser;
use likelike::{process_input, DummyWrap, HttpClientWrap as _, ReadLinkInformation, SqliteStore};

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    files: Vec<PathBuf>,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Args::parse();

    let store = SqliteStore::new().await;
    let store = DummyWrap::new(store);
    for file in cli.files {
        eprintln!("file={:?}", file);
        let data = read_to_string(file).await?;
        process_input(data.as_str(), &store).await?;
    }

    let mut iter = store.values().await?;
    while let Some(next) = iter.next().await {
        dbg!(next);
    }

    Ok(())
}
