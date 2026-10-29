//! The request surface shared by every front end (the Tauri app on each platform).
//! Method names and JSON shapes are what the UI's library interface calls.

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::{BookPatch, Choice, ImportMeta, ImportSource, Location, Node, RevRef};

#[derive(Deserialize)]
struct TicketParams {
    ticket: String,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct IdsParams {
    ids: Vec<String>,
    #[serde(default)]
    trashed: bool,
}

#[derive(Deserialize)]
struct UpdateParams {
    id: String,
    patch: BookPatch,
}

#[derive(Deserialize)]
struct LocationParams {
    id: String,
    location: Location,
}

#[derive(Deserialize)]
struct ResolveParams {
    id: String,
    shown: Vec<RevRef>,
    choice: Choice,
}

#[derive(Deserialize)]
struct ImportParams {
    path: std::path::PathBuf,
    meta: ImportMeta,
}

#[derive(Deserialize)]
struct SettingsParams {
    patch: serde_json::Map<String, Value>,
}

pub async fn dispatch(node: &Node, method: &str, params: Value) -> Result<Value> {
    let p = params;
    Ok(match method {
        "status" => serde_json::to_value(node.status())?,
        "outgoing" => serde_json::to_value(node.outgoing())?,
        "books" => serde_json::to_value(node.books())?,
        "importBook" => {
            let p: ImportParams = serde_json::from_value(p)?;
            let (book, added) = node.import_book(ImportSource::Path(p.path), p.meta).await?;
            serde_json::json!({ "book": book, "added": added })
        }
        "bookFile" => {
            let p: IdParams = serde_json::from_value(p)?;
            serde_json::to_value(node.book_file(&p.id)?)?
        }
        "onlyCopy" => {
            let p: IdParams = serde_json::from_value(p)?;
            serde_json::to_value(node.only_copy(&p.id)?)?
        }
        "removeDownload" => {
            let p: IdParams = serde_json::from_value(p)?;
            node.remove_download(p.id).await?;
            Value::Null
        }
        "restoreDownload" => {
            let p: IdParams = serde_json::from_value(p)?;
            node.restore_download(&p.id).await?;
            Value::Null
        }
        "downloads" => serde_json::to_value(node.downloads())?,
        "retryDownloads" => {
            node.retry_downloads();
            Value::Null
        }
        "updateBook" => {
            let p: UpdateParams = serde_json::from_value(p)?;
            serde_json::to_value(node.update_book(p.id, p.patch).await?)?
        }
        "setTrashed" => {
            let p: IdsParams = serde_json::from_value(p)?;
            serde_json::to_value(node.set_trashed(p.ids, p.trashed).await?)?
        }
        "deleteForever" => {
            let p: IdsParams = serde_json::from_value(p)?;
            serde_json::to_value(node.delete_forever(p.ids).await?)?
        }
        "openBook" => {
            let p: IdParams = serde_json::from_value(p)?;
            serde_json::to_value(node.open_book(p.id).await?)?
        }
        "peekPosition" => {
            let p: IdParams = serde_json::from_value(p)?;
            serde_json::to_value(node.peek_position(&p.id)?)?
        }
        "resolvePosition" => {
            let p: ResolveParams = serde_json::from_value(p)?;
            serde_json::to_value(node.resolve_position(p.id, p.shown, p.choice).await?)?
        }
        "checkpoint" => {
            let p: LocationParams = serde_json::from_value(p)?;
            serde_json::to_value(node.checkpoint(p.id, p.location).await?)?
        }
        "flushPosition" => {
            let p: LocationParams = serde_json::from_value(p)?;
            serde_json::to_value(node.flush_position(p.id, p.location).await?)?
        }
        "commitPosition" => {
            let p: IdParams = serde_json::from_value(p)?;
            node.commit_position(p.id).await?;
            Value::Null
        }
        "handoff" => {
            let p: IdParams = serde_json::from_value(p)?;
            serde_json::to_value(node.handoff(&p.id)?)?
        }
        "settings" => Value::Object(node.settings()),
        "setSettings" => {
            let p: SettingsParams = serde_json::from_value(p)?;
            Value::Object(node.set_settings(p.patch).await?)
        }
        "createInvite" => serde_json::to_value(node.create_invite()?)?,
        "join" => {
            let p: TicketParams = serde_json::from_value(p)?;
            serde_json::to_value(node.use_code(&p.ticket).await?)?
        }
        "networkChanged" => {
            node.network_changed().await;
            Value::Null
        }
        "removeDevice" => {
            let p: IdParams = serde_json::from_value(p)?;
            node.remove_device(p.id).await?;
            Value::Null
        }
        other => bail!("unknown method {other}"),
    })
}
