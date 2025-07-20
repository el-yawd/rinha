use shared_types::{GlobalSummary, Message, SledTree, Summary};
use sled::{self, Db, Tree};
use std::env;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task;

async fn handle_client(
    stream: UnixStream,
    default_tree: Tree,
    fallback_tree: Tree,
    db: Db,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader).lines();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<Message>(&line) {
            Ok(message) => match message {
                Message::Read { from, to } => {
                    let default =
                        Summary::from_iter(default_tree.range(from.as_str()..=to.as_str()));
                    let fallback =
                        Summary::from_iter(fallback_tree.range(from.as_str()..=to.as_str()));

                    let response = serde_json::to_string(&GlobalSummary { default, fallback })?;
                    writer.write_all(response.as_bytes()).await?;
                    writer.write_all(b"\n").await?;
                    writer.flush().await?;
                }
                Message::Write { key, value, tree } => {
                    let bytes = value.to_be_bytes();
                    match tree {
                        SledTree::Fallback => {
                            fallback_tree.insert(key.as_bytes(), &bytes)?;
                        }
                        SledTree::Default => {
                            default_tree.insert(key.as_bytes(), &bytes)?;
                        }
                    }
                }
                Message::Purge => {
                    db.drop_tree("fallback")?;
                    db.drop_tree("default")?;
                }
            },
            Err(e) => {
                eprintln!("Failed to parse message: {}", e);
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket_path = env::var("RINHADB_SOCK").unwrap_or("/tmp/rinha.sock".to_string());

    if Path::new(socket_path.as_str()).exists() {
        std::fs::remove_file(socket_path.clone())?;
    }

    let database_url: String = "app_db".to_string();

    let db = sled::open(database_url)?;
    let default_tree = db.open_tree("default")?;
    let fallback_tree = db.open_tree("fallback")?;

    let listener = UnixListener::bind(socket_path.as_str())?;
    println!("RinhaDB listening on {}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        let default_tree = default_tree.clone();
        let fallback_tree = fallback_tree.clone();
        let db = db.clone();

        task::spawn(async move {
            if let Err(e) = handle_client(stream, default_tree, fallback_tree, db).await {
                eprintln!("Error handling client: {}", e);
            }
        });
    }
}
