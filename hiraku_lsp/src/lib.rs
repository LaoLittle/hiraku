//! Small synchronous LSP server. JSON and stdio live here, not in the language VM.
//! Open document buffers are authoritative; the server never writes project files.
pub mod analysis;
use hiraku_script::{
    Stmt,
    cst::{SyntaxKind, SyntaxTree, statement_span},
    format::{FormatOptions, format_tree},
    source_text::{LineIndex, TextPosition},
    span::Span,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io};

struct Document {
    version: i64,
    tree: SyntaxTree,
}
#[derive(Default)]
pub struct Server {
    documents: BTreeMap<String, Document>,
    initialized: bool,
    shutdown: bool,
}

fn position(value: &Value) -> Option<TextPosition> {
    Some(TextPosition {
        line: value["line"].as_u64()?.try_into().ok()?,
        character: value["character"].as_u64()?.try_into().ok()?,
    })
}
fn wire_position(index: &LineIndex<'_>, offset: usize) -> Value {
    let p = index
        .position(offset)
        .expect("lexer and parser spans are UTF-8 boundaries");
    json!({"line":p.line,"character":p.character})
}
fn range(source: &str, span: Span) -> Value {
    let index = LineIndex::new(source);
    json!({"start":wire_position(&index,span.start),"end":wire_position(&index,span.end)})
}
fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn log_error(message: &str) -> Value {
    json!({"jsonrpc":"2.0","method":"window/logMessage","params":{"type":1,"message":message}})
}

impl Server {
    /// Returns outbound JSON-RPC messages. Requests and notifications share the
    /// same implementation used by the stdio binary and protocol tests.
    pub fn handle(&mut self, message: Value) -> Vec<Value> {
        let id = message.get("id").cloned();
        let Some(method) = message["method"].as_str() else {
            return vec![error(
                id.unwrap_or(Value::Null),
                -32600,
                "expected JSON-RPC method",
            )];
        };
        if message["jsonrpc"] != "2.0" {
            return vec![error(
                id.unwrap_or(Value::Null),
                -32600,
                "expected JSON-RPC 2.0",
            )];
        }
        if let Some(id) = id {
            match self.request(method, &message["params"]) {
                Ok(result) => vec![json!({"jsonrpc":"2.0","id":id,"result":result})],
                Err((code, text)) => vec![error(id, code, &text)],
            }
        } else {
            match self.notify(method, &message["params"]) {
                Ok(messages) => messages,
                Err(error) => vec![log_error(&error)],
            }
        }
    }
    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i32, String)> {
        if method == "initialize" {
            if self.initialized {
                return Err((-32600, "already initialized".into()));
            }
            self.initialized = true;
            return Ok(
                json!({"serverInfo":{"name":"hiraku-lsp","version":env!("CARGO_PKG_VERSION")},"capabilities":{
                    "positionEncoding":"utf-16", "textDocumentSync":{"openClose":true,"change":2},
                    "documentFormattingProvider":true,"documentSymbolProvider":true,"foldingRangeProvider":true
                }}),
            );
        }
        if !self.initialized {
            return Err((-32002, "server not initialized".into()));
        }
        if self.shutdown {
            return Err((-32600, "server is shutting down".into()));
        }
        if method == "shutdown" {
            self.shutdown = true;
            return Ok(Value::Null);
        }
        if !matches!(
            method,
            "textDocument/formatting" | "textDocument/documentSymbol" | "textDocument/foldingRange"
        ) {
            return Err((-32601, format!("method not supported: {method}")));
        }
        let document = params["textDocument"]["uri"]
            .as_str()
            .and_then(|uri| self.documents.get(uri))
            .ok_or((-32602, "document is not open".into()))?;
        let source = &document.tree.source;
        match method {
            "textDocument/formatting" => {
                let formatted = format_tree(
                    &document.tree,
                    FormatOptions {
                        indent_width: params["options"]["tabSize"]
                            .as_u64()
                            .unwrap_or(4)
                            .clamp(1, 16) as usize,
                        insert_spaces: params["options"]["insertSpaces"].as_bool().unwrap_or(true),
                    },
                )
                .map_err(|_| {
                    (
                        -32803,
                        "Fix syntax errors before formatting; buffer was not changed".into(),
                    )
                })?;
                if formatted == source.as_ref() {
                    return Ok(json!([]));
                }
                Ok(
                    json!([{"range":range(source,Span {start:0,end:source.len()}),"newText":formatted}]),
                )
            }
            "textDocument/documentSymbol" => {
                let mut symbols = Vec::new();
                if let Some(ast) = &document.tree.ast {
                    for statement in &ast.statements {
                        let (name, kind) = match statement {
                            Stmt::Function { name, .. } => (name, 12),
                            Stmt::Let { name, .. } | Stmt::Global { name, .. } => (name, 13),
                            Stmt::Struct { name, .. } => (name, 23),
                            Stmt::Enum { name, .. } => (name, 10),
                            Stmt::TypeAlias { name, .. } | Stmt::Protocol { name, .. } => {
                                (name, 11)
                            }
                            _ => continue,
                        };
                        let span = statement_span(statement);
                        let selection = document
                            .tree
                            .tokens
                            .iter()
                            .find(|token| {
                                token.span.start >= span.start
                                    && token.span.end <= span.end
                                    && document.tree.token_text(token) == name
                            })
                            .map_or(span, |token| token.span);
                        symbols.push(json!({"name":name,"kind":kind,"range":range(source,span),"selectionRange":range(source,selection)}));
                    }
                }
                Ok(Value::Array(symbols))
            }
            "textDocument/foldingRange" => {
                let index = LineIndex::new(source);
                Ok(Value::Array(document.tree.nodes.iter().filter(|node| matches!(node.kind,SyntaxKind::Braces|SyntaxKind::Brackets|SyntaxKind::Parentheses)).filter_map(|node| {
                    let start = index.position(node.span.start)?;
                    let end = index.position(node.span.end)?;
                    (end.line > start.line).then(|| json!({"startLine":start.line,"startCharacter":start.character,"endLine":end.line,"endCharacter":end.character}))
                }).collect()))
            }
            _ => unreachable!("supported methods checked above"),
        }
    }
    fn notify(&mut self, method: &str, params: &Value) -> Result<Vec<Value>, String> {
        if !self.initialized || self.shutdown {
            return Ok(Vec::new());
        }
        match method {
            "textDocument/didOpen" => {
                let doc = &params["textDocument"];
                let uri = doc["uri"].as_str().ok_or("missing document URI")?;
                let text = doc["text"].as_str().ok_or("missing document text")?;
                let version = doc["version"].as_i64().ok_or("missing document version")?;
                self.documents.insert(
                    uri.into(),
                    Document {
                        version,
                        tree: SyntaxTree::parse(text),
                    },
                );
                Ok(vec![self.diagnostics(uri)])
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"]
                    .as_str()
                    .ok_or("missing document URI")?;
                let version = params["textDocument"]["version"]
                    .as_i64()
                    .ok_or("missing document version")?;
                let doc = self.documents.get_mut(uri).ok_or("document is not open")?;
                if version <= doc.version {
                    return Ok(Vec::new());
                }
                let mut text = doc.tree.source.to_string();
                for change in params["contentChanges"]
                    .as_array()
                    .ok_or("missing contentChanges")?
                {
                    let replacement = change["text"].as_str().ok_or("missing replacement text")?;
                    if let Some(edit) = change.get("range") {
                        let index = LineIndex::new(&text);
                        let start = position(&edit["start"])
                            .and_then(|p| index.offset(p))
                            .ok_or("invalid UTF-16 edit start")?;
                        let end = position(&edit["end"])
                            .and_then(|p| index.offset(p))
                            .ok_or("invalid UTF-16 edit end")?;
                        if start > end {
                            return Err("reversed edit range".into());
                        }
                        text.replace_range(start..end, replacement);
                    } else {
                        text = replacement.into();
                    }
                }
                *doc = Document {
                    version,
                    tree: SyntaxTree::parse(text),
                };
                Ok(vec![self.diagnostics(uri)])
            }
            "textDocument/didClose" => {
                let uri = params["textDocument"]["uri"]
                    .as_str()
                    .ok_or("missing document URI")?;
                self.documents.remove(uri);
                Ok(vec![
                    json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"diagnostics":[]}}),
                ])
            }
            _ => Ok(Vec::new()),
        }
    }
    fn diagnostics(&self, uri: &str) -> Value {
        let doc = &self.documents[uri];
        let diagnostics: Vec<_> = analysis::diagnostics(&doc.tree).into_iter().map(|diagnostic| {
            let severity = match diagnostic.severity { analysis::Severity::Error => 1, analysis::Severity::Warning => 2 };
            json!({"range":range(&doc.tree.source,diagnostic.span),"severity":severity,"code":diagnostic.code,"source":"hiraku","message":diagnostic.message})
        }).collect();
        json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"version":doc.version,"diagnostics":diagnostics}})
    }
}

/// The transport owns framing and IO threads; the document service owns state.
/// Responses to server-initiated requests can be handled here when added.
pub fn serve_connection(connection: lsp_server::Connection) -> io::Result<()> {
    let mut server = Server::default();
    for message in &connection.receiver {
        if matches!(&message, lsp_server::Message::Response(_)) {
            continue;
        }
        if matches!(&message, lsp_server::Message::Notification(notification) if notification.method == "exit")
        {
            return if server.shutdown {
                Ok(())
            } else {
                Err(io::Error::other("exit before shutdown"))
            };
        }
        let mut input = serde_json::to_value(message).map_err(io::Error::other)?;
        input["jsonrpc"] = json!("2.0");
        for output in server.handle(input) {
            let message = serde_json::from_value(output).map_err(io::Error::other)?;
            connection.sender.send(message).map_err(io::Error::other)?;
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "client disconnected before exit",
    ))
}

/// Stdio is provided by lsp-server, not a second framing implementation.
pub fn serve_stdio() -> io::Result<()> {
    let (connection, threads) = lsp_server::Connection::stdio();
    let result = serve_connection(connection);
    // On protocol failure the reader may still be blocked on stdin. Do not wait
    // for it; the binary exits with an error. On normal exit, drain the writer.
    result?;
    threads.join()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn server() -> Server {
        let mut s = Server::default();
        s.handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
        s
    }
    #[test]
    fn incremental_edits_are_utf16_versioned_and_transactional() {
        let mut s = server();
        let uri = "file:///sample.hks";
        s.handle(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"version":1,"text":"let alice = \"😀\""}}}));
        let change = |version, start, end, text| json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":version},"contentChanges":[{"range":{"start":{"line":0,"character":start},"end":{"line":0,"character":end}},"text":text}]}});
        assert_eq!(
            s.handle(change(2, 13, 14, "Bob"))[0]["method"],
            "window/logMessage"
        );
        assert_eq!(s.documents[uri].version, 1);
        s.handle(change(2, 13, 15, "Bob"));
        assert_eq!(s.documents[uri].tree.source.as_ref(), "let alice = \"Bob\"");
        s.handle(change(1, 0, 3, "var"));
        assert_eq!(s.documents[uri].version, 2);
    }
    #[test]
    fn diagnostics_formatting_symbols_and_close() {
        let mut s = server();
        let uri = "file:///sample.hks";
        let open = |text| json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"version":1,"text":text}}});
        assert!(
            !s.handle(open("let alice ="))[0]["params"]["diagnostics"]
                .as_array()
                .expect("diagnostics")
                .is_empty()
        );
        s.handle(open("fn greet() {\n\"Hello\"\n}"));
        let request = |method| json!({"jsonrpc":"2.0","id":2,"method":method,"params":{"textDocument":{"uri":uri},"options":{"tabSize":2,"insertSpaces":true}}});
        assert!(
            s.handle(request("textDocument/formatting"))[0]["result"][0]["newText"]
                .as_str()
                .expect("edit")
                .contains("  \"Hello\"")
        );
        assert_eq!(
            s.handle(request("textDocument/documentSymbol"))[0]["result"][0]["name"],
            "greet"
        );
        assert_eq!(
            s.handle(request("textDocument/foldingRange"))[0]["result"][0]["endLine"],
            2
        );
        s.handle(json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri}}}));
        assert!(s.documents.is_empty());
    }

    #[test]
    fn embedded_and_protocol_diagnostics_share_ranges_and_messages() {
        let source = "let alice = \"😀\"\nlet bob =";
        let embedded = analysis::analyze(source, None);
        let mut server = server();
        let published = server.handle(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"untitled:sample.hks","version":1,"text":source}}}));
        let diagnostics = published[0]["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics");
        assert_eq!(diagnostics.len(), embedded.diagnostics.len());
        for (wire, typed) in diagnostics.iter().zip(embedded.diagnostics) {
            assert_eq!(wire["message"], typed.message);
            assert_eq!(wire["range"], range(source, typed.span));
        }
    }
    #[test]
    fn memory_connection_delivers_requests_and_shutdown() {
        let (client, server) = lsp_server::Connection::memory();
        let worker = std::thread::spawn(|| serve_connection(server));
        for value in [
            json!({"id":1,"method":"initialize","params":{}}),
            json!({"method":"initialized","params":{}}),
            json!({"id":2,"method":"shutdown","params":null}),
            json!({"method":"exit","params":null}),
        ] {
            client
                .sender
                .send(serde_json::from_value(value).expect("message"))
                .expect("send");
        }
        let first = client
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("initialize");
        assert_eq!(
            serde_json::to_value(first).expect("response")["result"]["capabilities"]["positionEncoding"],
            "utf-16"
        );
        let second = client
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("shutdown");
        assert_eq!(serde_json::to_value(second).expect("response")["id"], 2);
        worker.join().expect("worker").expect("clean shutdown");
    }

    #[test]
    fn exit_before_shutdown_is_an_error() {
        let (client, server) = lsp_server::Connection::memory();
        client
            .sender
            .send(lsp_server::Notification::new("exit".into(), ()).into())
            .expect("send exit");
        assert!(serve_connection(server).is_err());
    }
}
