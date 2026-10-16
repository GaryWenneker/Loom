use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

fn setup() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let call = |args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_loom"))
            .current_dir(dir.path())
            .env_remove("LOOM_OBJECT_DIR")
            .env_remove("LOOM_OBJECT_DB")
            .env("LOOM_AUTHOR", "test")
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    };
    call(&["init"]);
    fs::write(dir.path().join("binary"), [0, 255, 10, 13, 0]).unwrap();
    fs::write(dir.path().join("plain"), b"hello").unwrap();
    let output = call(&["commit", "-m", "batch base"]);
    let revision = String::from_utf8(output)
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap()
        .to_owned();
    (dir, revision)
}
fn batch(root: &std::path::Path, revision: &str, input: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env_remove("LOOM_OBJECT_DIR")
        .env_remove("LOOM_OBJECT_DB")
        .args(["query", "--batch", &format!("@{revision}")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let data = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let result = child.wait_with_output().unwrap();
    writer.join().unwrap();
    result
}
fn header(output: &[u8], offset: &mut usize) -> Value {
    let end = output[*offset..].iter().position(|b| *b == b'\n').unwrap() + *offset;
    let value = serde_json::from_slice(&output[*offset..end]).unwrap();
    *offset = end + 1;
    value
}
#[test]
fn batch_is_binary_framed_and_continues_after_request_errors() {
    let (dir, revision) = setup();
    let requests = [
        json!({"id":1,"op":"cat","path":"binary"}),
        json!({"id":2,"op":"cat","path":"missing"}),
        json!({"id":3,"op":"cat","path":"../unsafe"}),
        json!({"id":4,"op":"files","limit":1}),
        json!({"id":5,"op":"files","after":"binary","limit":1}),
        json!({"id":6,"op":"stats"}),
    ];
    let input = requests
        .iter()
        .map(|v| format!("{v}\n"))
        .collect::<String>();
    let result = batch(dir.path(), &revision, input.as_bytes());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut offset = 0;
    let first = header(&result.stdout, &mut offset);
    assert_eq!(first["size"], 5);
    assert_eq!(
        &result.stdout[offset..offset + 6],
        &[0, 255, 10, 13, 0, b'\n']
    );
    offset += 6;
    assert_eq!(header(&result.stdout, &mut offset)["kind"], "missing");
    assert_eq!(header(&result.stdout, &mut offset)["ok"], false);
    let page = header(&result.stdout, &mut offset);
    assert_eq!(page["paths"], json!(["binary"]));
    assert_eq!(page["next_cursor"], "binary");
    let page = header(&result.stdout, &mut offset);
    assert_eq!(page["paths"], json!(["plain"]));
    assert!(page["next_cursor"].is_null());
    let stats = header(&result.stdout, &mut offset);
    assert_eq!(stats["kind"], "stats");
    assert!(stats["cache_hits"].as_u64().unwrap() > 0);
    assert_eq!(offset, result.stdout.len());
}
#[test]
fn malformed_and_unknown_fields_are_rejected_without_losing_framing() {
    let (dir, revision) = setup();
    let result = batch(
        dir.path(),
        &revision,
        b"not json\n{\"op\":\"stats\",\"id\":1,\"surprise\":true}\n{\"op\":\"stats\",\"id\":2}\n",
    );
    assert!(result.status.success());
    let lines: Vec<Value> = result
        .stdout
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["ok"], false);
    assert_eq!(lines[1]["ok"], false);
    assert_eq!(lines[2]["id"], 2);
}
#[test]
fn oversized_requests_fail_with_a_bound_instead_of_unbounded_allocation() {
    let (dir, revision) = setup();
    let result = batch(dir.path(), &revision, &vec![b'x'; 65537]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("64 KiB"));
}

#[test]
fn bulk_reply_preserves_binary_boundaries_missing_paths_and_request_order() {
    let (dir, revision) = setup();
    let input = b"{\"id\":1,\"op\":\"cat_many\",\"paths\":[\"binary\",\"missing\",\"plain\",\"binary\"],\"max_bytes\":15}\n{\"id\":2,\"op\":\"cat\",\"path\":\"plain\"}\n";
    let result = batch(dir.path(), &revision, input);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut offset = 0;
    let reply = header(&result.stdout, &mut offset);
    assert_eq!(reply["kind"], "blobs");
    assert_eq!(reply["size"], 15);
    assert_eq!(
        reply["entries"],
        json!([{"size":5},null,{"size":5},{"size":5}])
    );
    let mut expected = vec![0, 255, 10, 13, 0];
    expected.extend_from_slice(b"hello");
    expected.extend_from_slice(&[0, 255, 10, 13, 0, b'\n']);
    assert_eq!(&result.stdout[offset..offset + 16], expected);
    offset += 16;
    assert_eq!(header(&result.stdout, &mut offset)["id"], 2);
    assert_eq!(&result.stdout[offset..], b"hello\n");
}

#[test]
fn bulk_failures_emit_no_partial_payload_and_allow_next_requests() {
    let (dir, revision) = setup();
    let mut requests = vec![
        json!({"id":1,"op":"cat_many","paths":["binary","plain"],"max_bytes":9}),
        json!({"id":2,"op":"cat_many","paths":["binary","../unsafe"]}),
        json!({"id":3,"op":"cat_many","paths":vec!["missing";4097]}),
        json!({"id":4,"op":"cat_many","paths":[]}),
    ]
    .into_iter()
    .map(|v| format!("{v}\n"))
    .collect::<String>();
    requests.push_str("{\"id\":5,\"op\":\"stats\"}\n");
    let result = batch(dir.path(), &revision, requests.as_bytes());
    assert!(result.status.success());
    let mut offset = 0;
    for id in 1..=3 {
        let reply = header(&result.stdout, &mut offset);
        assert_eq!(reply["id"], id);
        assert_eq!(reply["ok"], false);
    }
    let empty = header(&result.stdout, &mut offset);
    assert_eq!(empty["entries"], json!([]));
    assert_eq!(empty["size"], 0);
    assert_eq!(result.stdout[offset], b'\n');
    offset += 1;
    assert_eq!(header(&result.stdout, &mut offset)["id"], 5);
    assert_eq!(offset, result.stdout.len());
}

#[test]
fn bulk_and_scalar_replies_flush_while_input_remains_open() {
    use std::io::{BufRead, Read};
    let (dir, revision) = setup();
    let mut child = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(dir.path())
        .env_remove("LOOM_OBJECT_DIR")
        .env_remove("LOOM_OBJECT_DB")
        .args(["query", "--batch", &format!("@{revision}")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut output = std::io::BufReader::new(output);
        for _ in 0..2 {
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            let header: Value = serde_json::from_str(&line).unwrap();
            let mut payload = vec![0; header["size"].as_u64().unwrap() as usize + 1];
            output.read_exact(&mut payload).unwrap();
            send.send((header, payload)).unwrap();
        }
    });
    for (id, op) in [(1, "cat_many"), (2, "cat")] {
        let request = if op == "cat_many" {
            json!({"id":id,"op":op,"paths":["plain"]})
        } else {
            json!({"id":id,"op":op,"path":"plain"})
        };
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        let result = receive.recv_timeout(std::time::Duration::from_secs(5));
        if result.is_err() {
            child.kill().unwrap();
        }
        let (header, bytes) = result.expect("reply must arrive before input EOF");
        assert_eq!(header["id"], id);
        assert_eq!(bytes, b"hello\n");
    }
    drop(input);
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
}
