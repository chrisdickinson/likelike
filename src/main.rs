use futures::{future::join_all, StreamExt};
use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use likelike::{
    process_input, Frontmatter, HttpClientWrap, LinkSource, ReadLinkInformation, SqliteStore,
};

/// Process markdown-formatted linkdump files and store them in a sqlite database.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,

    /// If not given, defaults to the local data dir per the "dirs" crate. E.g., on macOS, this
    /// will be "sqlite:///Users/foo/Library/Application Support/likelike.sqlite3".
    #[arg(short, long)]
    database_url: Option<String>,
}

#[derive(Default, Clone, Copy, Debug, ValueEnum)]
enum ShowMode {
    Text,
    Source,

    #[cfg(feature = "llm")]
    Summary,

    #[default]
    Metadata,
}

impl std::fmt::Display for ShowMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShowMode::Text => f.write_str("text"),
            ShowMode::Source => f.write_str("source"),
            ShowMode::Metadata => f.write_str("metadata"),
            #[cfg(feature = "llm")]
            ShowMode::Summary => f.write_str("summary"),
        }
    }
}

#[derive(Parser, Debug)]
enum Commands {
    /// Import links from a set of files. Files are parsed for nested markdown lists containing
    /// links in markdown-anchor or "text: url" format.
    ///
    /// Sublists are used to add metadata.
    ///
    /// E.g.:
    ///
    /// ```
    /// - some link: https://foo.bar/baz
    ///   - notes:
    ///     - # heading
    ///     - some more text
    ///   - tags:
    ///     - a
    ///     - b
    /// ```
    ///
    Import {
        files: Vec<PathBuf>,

        /// Pass this argument to display imported link data.
        #[arg(long)]
        display_links: bool,
    },

    /// Export links from the database as zola markdown documents with Link metadata included in
    /// frontmatter.
    Export { output: PathBuf },

    /// Show information about a given link. Accepts globstar patterns (be sure to single-quote
    /// them!)
    Show {
        url: String,

        /// Only show metadata information: meta tags & http headers
        #[arg(short, long, default_value_t=ShowMode::Metadata)]
        mode: ShowMode,
    },
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
        Commands::Show { url, mode } => {
            let mut links = store.glob(url.as_str()).await?;

            while let Some(link) = links.next().await {
                match mode {
                    ShowMode::Text => {
                        if let Some(src) = link.extract_text() {
                            println!("{}", src);
                        }
                    }

                    ShowMode::Source => {
                        if let Some(src) = link.src() {
                            std::io::stdout().write_all(src)?;
                        }
                    }

                    ShowMode::Metadata => {
                        // link filename found / read / fetched / processed size
                        // - via: <via>
                        // - tags: [alpha,beta,gamma]
                        // - meta:
                        //   - prop: value
                        // - headers:
                        //   - header: data
                        //   - header: data
                        //   - header: data
                        println!("{}", link.url());
                        if let Some(filename) = link.from_filename() {
                            let homedir = dirs::home_dir().unwrap();
                            println!(
                                "- from: {}",
                                filename.replace(homedir.as_path().to_str().unwrap(), "~")
                            );
                        }

                        let found_at = link.found_at().map(|t| {
                            t.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %l:%M%P")
                                .to_string()
                                .replace("  ", " ")
                        });
                        let read_at = link.read_at().map(|t| {
                            t.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %l:%M%P")
                                .to_string()
                                .replace("  ", " ")
                        });
                        let fetched = link.last_fetched().map(|t| {
                            t.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %l:%M%P")
                                .to_string()
                                .replace("  ", " ")
                        });
                        let processed = link.last_processed().map(|t| {
                            t.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %l:%M%P")
                                .to_string()
                                .replace("  ", " ")
                        });

                        let times: std::collections::BTreeSet<_> = itertools::chain!(
                            found_at.as_deref(),
                            read_at.as_deref(),
                            fetched.as_deref(),
                            processed.as_deref()
                        )
                        .collect();
                        let mut actions = Vec::with_capacity(4);
                        for time in times {
                            actions.clear();
                            if found_at.as_deref() == Some(time) {
                                actions.push("found");
                            }
                            if read_at.as_deref() == Some(time) {
                                actions.push("read");
                            }
                            if fetched.as_deref() == Some(time) {
                                actions.push("fetched");
                            }
                            if processed.as_deref() == Some(time) {
                                actions.push("processed");
                            }

                            println!("- {}: {}", itertools::join(&actions, ";"), time);
                        }

                        if let Some(via) = link.via() {
                            println!(
                                "- via: {}",
                                match via {
                                    likelike::Via::Friend(xs) => format!("friend, {}", xs),
                                    likelike::Via::Link(xs) => format!("link, {}", xs),
                                    likelike::Via::Freeform(xs) => format!("text, {}", xs),
                                }
                            );
                        }

                        for (name, map) in itertools::chain(
                            link.meta().map(|x| ("meta", x)),
                            link.http_headers().map(|x| ("headers", x)),
                        ) {
                            if map.is_empty() {
                                continue;
                            }

                            println!("- {}:", name);
                            let map: std::collections::BTreeMap<_, _> = map.iter().collect();
                            for (key, value) in map {
                                let value = itertools::join(value, ", ");
                                println!(
                                    "  - {}: {}",
                                    if key.contains(':') {
                                        format!("\"{}\"", key)
                                    } else {
                                        key.to_string()
                                    },
                                    if value.contains('\n') {
                                        format!(
                                            "|\n    {}",
                                            itertools::join(value.split('\n'), "\n    ")
                                        )
                                    } else {
                                        value
                                    }
                                );
                            }
                        }
                    }

                    #[cfg(feature = "llm")]
                    ShowMode::Summary => {
                        use itertools::Itertools;
                        use llm::{samplers::TopPTopK, Model, ModelParameters};
                        use std::sync::Arc;

                        if let Some(src) = link.extract_text() {
                            let ggml = std::env::var("LIKELIKE_GGML")
                                .ok()
                                .unwrap_or_else(|| "ggml-vicuna-13B-1.1-q5_1.bin".to_string());
                            // load a GGML model from disk
                            let llama = llm::load::<llm::models::Llama>(
                                // path to GGML file
                                std::path::Path::new(ggml.as_str()),
                                llm::VocabularySource::Model,
                                // llm::ModelParameters
                                ModelParameters {
                                    context_size: 8192,
                                    ..Default::default()
                                },
                                // load progress callback
                                // llm::load_progress_callback_stdout
                                |_| {},
                            )
                            .unwrap_or_else(|err| panic!("Failed to load model: {err}"));

                            // use the model to generate text from a prompt
                            let mut session = llama.start_session(Default::default());

                            let src: String = itertools::join(
                                src.lines().filter(|xs| {
                                    !xs.starts_with('[')
                                        && !xs.starts_with('#')
                                        && !xs.trim().is_empty()
                                }),
                                "\n",
                            );
                            let src: String = src.replace(|c| matches!(c, '*' | '\u{fffd}'), "");

                            let mut output = Vec::with_capacity(2048);
                            let paras: Vec<_> = src.split('\n').collect();
                            for paras in &paras.into_iter().chunks(4) {
                                let next_paras = itertools::join(paras, "\n");

                                let prompt = format!(
                                    indoc::indoc! {r#"Below is an instruction that describes a task. Write a response that appropriately completes the request.

                                ### Instruction:

                                Write a concise, 100 word summary of the following markdown text.

                                #### Text:
                                {}

                                ### Response:
                                "#},
                                    next_paras
                                );
                                output.clear();

                                let res = session.infer::<std::convert::Infallible>(
                                    // model to use for text generation
                                    &llama,
                                    // randomness provider
                                    &mut rand::thread_rng(),
                                    // the prompt to use for text generation, as well as other
                                    // inference parameters
                                    &llm::InferenceRequest {
                                        prompt: prompt.as_str().into(),
                                        parameters: &llm::InferenceParameters {
                                            sampler: Arc::new(TopPTopK {
                                                top_k: 40,
                                                top_p: 0.95,
                                                repeat_penalty: 1.30,
                                                temperature: 0.50,
                                                repetition_penalty_last_n: 512,
                                                ..Default::default()
                                            }),
                                            ..Default::default()
                                        },
                                        play_back_previous_tokens: true,
                                        maximum_token_count: None,
                                    },
                                    // llm::OutputRequest
                                    &mut Default::default(),
                                    // output callback
                                    |r| match r {
                                        llm::InferenceResponse::PromptToken(t) => {
                                            print!("\x1b[31m{t}\x1b[0m");
                                            std::io::stdout().flush().unwrap();
                                            Ok(llm::InferenceFeedback::Continue)
                                        }

                                        llm::InferenceResponse::InferredToken(t) => {
                                            print!("\x1b[34m{}\x1b[0m", t.as_str());
                                            output.push(t);
                                            std::io::stdout().flush().unwrap();
                                            if output.len() > 128 {
                                                Ok(llm::InferenceFeedback::Halt)
                                            } else {
                                                Ok(llm::InferenceFeedback::Continue)
                                            }
                                        }

                                        _ => Ok(llm::InferenceFeedback::Continue),
                                    },
                                );

                                match res {
                                    Ok(result) => println!("\n\nInference stats:\n{result}"),
                                    Err(err) => println!("\naw heck {err}"),
                                }
                            }
                        }
                    }
                }
            }
        }

        Commands::Export { output } => {
            let mut links = store.values().await?;

            while let Some(link) = links.next().await {
                if link.read_at().is_none() {
                    continue;
                }

                let Ok(frontmatter): Result<Frontmatter, _> = link.try_into() else { continue };
                let mut path = output.clone();
                path.push(frontmatter.filename());
                std::fs::write(
                    path,
                    format!(
                        "+++\n{}\n+++\n{}",
                        toml::to_string_pretty(&frontmatter)?,
                        frontmatter.notes()
                    ),
                )?;
            }
        }

        Commands::Import {
            files,
            display_links,
        } => {
            let store = HttpClientWrap::wrap(store);
            let store = &store;

            let mut resolved_files = Vec::new();
            _find_markdown_files(&mut resolved_files, files, FindMode::Explicit)?;

            let mut futs = Vec::with_capacity(resolved_files.len());
            for file in resolved_files.into_iter() {
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

            if display_links {
                let mut links = store.values().await?;

                while let Some(link) = links.next().await {
                    eprintln!("{:?}", link);
                }
            }
        }
    }

    Ok(())
}

#[derive(PartialEq)]
enum FindMode {
    Explicit,
    Implicit,
}

fn _find_markdown_files(
    output: &mut Vec<PathBuf>,
    files: Vec<PathBuf>,
    mode: FindMode,
) -> eyre::Result<()> {
    output.reserve(files.len());
    for file in files.into_iter() {
        let Ok(metadata) = std::fs::metadata(file.as_path()) else { continue };
        if metadata.is_dir() {
            let entries: Vec<_> = std::fs::read_dir(file.as_path())?
                .filter_map(|file| Some(file.ok()?.path()))
                .collect();

            _find_markdown_files(output, entries, FindMode::Implicit)?;
        } else if mode == FindMode::Implicit {
            // filter "implicit" files by extension.
            let Some(ext) = file.extension() else { continue };

            if "md" == ext {
                output.push(file);
            }
        } else {
            output.push(file);
        }
    }

    Ok(())
}
