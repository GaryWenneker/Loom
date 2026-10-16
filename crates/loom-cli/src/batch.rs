use loom_core::Snapshot;
use serde::Deserialize;
use serde_json::json;
use std::io::{BufRead, Read, Write};

const MAX_REQUEST_BYTES: u64 = 64 * 1024;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Cat {
        id: u64,
        path: String,
    },
    Files {
        id: u64,
        prefix: Option<String>,
        after: Option<String>,
        limit: Option<usize>,
    },
    Stats {
        id: u64,
    },
}
impl Request {
    fn id(&self) -> u64 {
        match self {
            Self::Cat { id, .. } | Self::Files { id, .. } | Self::Stats { id } => *id,
        }
    }
}

/// Protocol v1: one bounded JSON request per line. A successful cat reply has
/// one JSON header, exactly `size` binary bytes and one LF. Other replies are
/// JSON-only. Flush per request so interactive clients cannot deadlock.
pub fn serve(
    snapshot: &Snapshot,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = input;
    loop {
        let mut line = Vec::new();
        let count = input
            .by_ref()
            .take(MAX_REQUEST_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        if count as u64 > MAX_REQUEST_BYTES {
            return Err("batch request exceeds 64 KiB limit".into());
        }
        let request: Request = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(error) => {
                serde_json::to_writer(
                    &mut output,
                    &json!({"id":null,"ok":false,"kind":"error","protocol":1,"error":error.to_string()}),
                )?;
                output.write_all(b"\n")?;
                output.flush()?;
                continue;
            }
        };
        let id = request.id();
        let result = match request {
            Request::Cat { path, .. } => snapshot.read_file(&path).map(|data| match data {
                Some(data) => (json!({"id":id,"ok":true,"kind":"blob","protocol":1,"size":data.len()}), Some(data)),
                None => (json!({"id":id,"ok":true,"kind":"missing","protocol":1}), None),
            }),
            Request::Files { prefix, after, limit, .. } => snapshot.files(prefix.as_deref(), after.as_deref(), limit.unwrap_or(10_000))
                .map(|page| (json!({"id":id,"ok":true,"kind":"files","protocol":1,"paths":page.paths,"next_cursor":page.next_cursor}), None)),
            Request::Stats { .. } => snapshot.stats().map(|stats| (json!({
                "id":id,"ok":true,"kind":"stats","protocol":1,"root":snapshot.root().to_string(),
                "tree_reads":stats.tree_reads,"blob_reads":stats.blob_reads,"cache_hits":stats.cache_hits,
                "cache_bytes":stats.cache_bytes,"cache_entries":stats.cache_entries,
            }), None)),
        };
        let (header, data) = match result {
            Ok(value) => value,
            Err(error) => (
                json!({"id":id,"ok":false,"kind":"error","protocol":1,"error":error.to_string()}),
                None,
            ),
        };
        serde_json::to_writer(&mut output, &header)?;
        output.write_all(b"\n")?;
        if let Some(data) = data {
            output.write_all(&data)?;
            output.write_all(b"\n")?;
        }
        output.flush()?;
    }
}
