#![cfg(feature = "cli")]

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Server {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    root: PathBuf,
    next_id: u64,
}

impl Server {
    fn start() -> Self {
        let root = std::env::temp_dir().join(format!(
            "cua-lsp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join(".emmyrc.json"),
            r#"{"strict":{"checkUnknown":true,"noImplicitAny":true}}"#,
        )
        .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_emmylua_ls"))
            .args([
                "--resources-path",
                "none",
                "--load-stdlib",
                "false",
                "--log-level",
                "error",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let mut size = None;
                loop {
                    let mut line = String::new();
                    if stdout.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(length) = line.strip_prefix("Content-Length:") {
                        size = length.trim().parse::<usize>().ok();
                    }
                }
                let Some(size) = size else {
                    return;
                };
                let mut bytes = vec![0; size];
                if stdout.read_exact(&mut bytes).is_err() {
                    return;
                }
                let Ok(value) = serde_json::from_slice(&bytes) else {
                    return;
                };
                if sender.send(value).is_err() {
                    return;
                }
            }
        });
        let mut server = Self {
            child,
            stdin,
            messages,
            root,
            next_id: 1,
        };
        let root_uri = emmylua_code_analysis::file_path_to_uri(&server.root).unwrap();
        server.request("initialize", json!({"processId":null,"rootUri":root_uri.as_str(),"workspaceFolders":[{"uri":root_uri.as_str(),"name":"cua"}],"capabilities":{"textDocument":{"diagnostic":{}},"workspace":{"configuration":true}}}));
        server.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
        server
    }

    fn send(&mut self, value: Value) {
        let bytes = serde_json::to_vec(&value).unwrap();
        write!(self.stdin, "Content-Length: {}\r\n\r\n", bytes.len()).unwrap();
        self.stdin.write_all(&bytes).unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self
                .messages
                .recv_timeout(remaining)
                .expect("language server response");
            if message.get("method").is_some() && message.get("id").is_some() {
                let result = if message["method"] == "workspace/configuration" {
                    Value::Array(
                        message["params"]["items"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|_| json!({}))
                            .collect(),
                    )
                } else {
                    Value::Null
                };
                self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":result}));
            } else if message["id"] == id {
                assert!(message.get("error").is_none(), "{message}");
                return message["result"].clone();
            }
        }
    }

    fn wait_diagnostics(&mut self, uri: &str, code: Option<&str>) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let result = self.request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            );
            if let Some(items) = result["items"].as_array() {
                let matched = match code {
                    Some(code) => items.iter().any(|item| item["code"] == code),
                    None => items
                        .iter()
                        .all(|item| item["severity"].as_u64().unwrap_or(4) > 2),
                };
                if matched {
                    return result;
                }
            }
            assert!(
                Instant::now() < deadline,
                "diagnostics did not update: {result}"
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn opens_changes_and_hovers_cua_over_lsp() {
    let mut server = Server::start();
    let uri = emmylua_code_analysis::file_path_to_uri(&server.root.join("camera.cua"))
        .unwrap()
        .to_string();
    server.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"lua","version":1,"text":"local value: number = 'bad'\nreturn value"}}}));
    let result = server.wait_diagnostics(&uri, Some("assign-type-mismatch"));
    let issue = result["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "assign-type-mismatch")
        .unwrap();
    assert_eq!(issue["range"]["start"]["line"], 0);
    assert_eq!(issue["range"]["start"]["character"], 22);
    server.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"local value:"}]}}));
    server.wait_diagnostics(&uri, Some("cua-syntax-error"));
    let tokens = server.request(
        "textDocument/semanticTokens/full",
        json!({"textDocument":{"uri":uri}}),
    );
    assert_eq!(tokens["data"], json!([]));
    server.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":3},"contentChanges":[{"text":"local value: string = 'ok'\nreturn value"}]}}));
    server.wait_diagnostics(&uri, None);
    let hover = server.request(
        "textDocument/hover",
        json!({"textDocument":{"uri":uri},"position":{"line":1,"character":9}}),
    );
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("string"),
        "{hover}"
    );
    assert_eq!(hover["range"]["start"]["line"], 1);
    server.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":4},"contentChanges":[{"text":"local value: unknown = 'ok'\nlocal text = value as string\nreturn text"}]}}));
    server.wait_diagnostics(&uri, None);
    let hover = server.request(
        "textDocument/hover",
        json!({"textDocument":{"uri":uri},"position":{"line":1,"character":24}}),
    );
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("string"),
        "{hover}"
    );
    assert_eq!(hover["range"]["start"]["line"], 1);
    assert_eq!(hover["range"]["start"]["character"], 22, "{hover}");
    assert_eq!(hover["range"]["end"]["character"], 28);
    let hover = server.request(
        "textDocument/hover",
        json!({"textDocument":{"uri":uri},"position":{"line":2,"character":9}}),
    );
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("string"),
        "{hover}"
    );
    let semantic = server.request(
        "textDocument/semanticTokens/full",
        json!({"textDocument":{"uri":uri}}),
    );
    let data = semantic["data"].as_array().expect("semantic token data");
    let mut line = 0;
    let mut column = 0;
    let mut types = Vec::new();
    for token in data.chunks_exact(5) {
        let delta_line = token[0].as_u64().unwrap();
        line += delta_line;
        column = if delta_line == 0 {
            column + token[1].as_u64().unwrap()
        } else {
            token[1].as_u64().unwrap()
        };
        if token[3] == 1 {
            types.push((
                line,
                column,
                token[2].as_u64().unwrap(),
                token[4].as_u64().unwrap(),
            ));
        }
    }
    assert!(
        types
            .iter()
            .any(|&(line, col, len, _)| (line, col, len) == (0, 13, 7)),
        "{types:?}"
    );
    assert!(
        types
            .iter()
            .any(|&(line, col, len, _)| (line, col, len) == (1, 22, 6)),
        "{types:?}"
    );
    let definition = server.request(
        "textDocument/definition",
        json!({"textDocument":{"uri":uri},"position":{"line":2,"character":9}}),
    );
    let location = if definition.is_array() {
        &definition[0]
    } else {
        &definition
    };
    assert_eq!(location["uri"], uri);
    assert_eq!(location["range"]["start"], json!({"line":1,"character":6}));
    server.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":5},"contentChanges":[{"text":"local n: number = 1\n---@class Camera\nlocal Camera = {}\nlocal camera: Camera = {}\nreturn camera"}]}}));
    server.wait_diagnostics(&uri, None);
    let definition = server.request(
        "textDocument/typeDefinition",
        json!({"textDocument":{"uri":uri},"position":{"line":4,"character":9}}),
    );
    assert_eq!(definition[0]["uri"], uri);
    assert_eq!(
        definition[0]["range"]["start"],
        json!({"line":1,"character":10})
    );
    let formatting = server.request(
        "textDocument/formatting",
        json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":true}}),
    );
    assert!(formatting.is_null());
    server.send(json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri}}}));
    server.request("shutdown", Value::Null);
    server.send(json!({"jsonrpc":"2.0","method":"exit"}));
}
