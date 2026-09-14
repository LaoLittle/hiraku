use serde_json::json;
use std::{
    io::Cursor,
    process::{Command, Stdio},
};

#[test]
fn binary_speaks_lsp_on_stdout_and_shuts_down_cleanly() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hiraku-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start language server");
    let mut stdin = child.stdin.take().expect("stdin pipe");
    for message in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///alice.hks","languageId":"hks","version":1,"text":"let alice ="}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ] {
        serde_json::from_value::<lsp_server::Message>(message)
            .expect("LSP message")
            .write(&mut stdin)
            .expect("send request");
    }
    // Keep stdin open: `exit` must terminate the transport without requiring EOF.
    let result = child.wait_with_output().expect("server finishes");
    drop(stdin);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut output = Cursor::new(result.stdout);
    assert_eq!(
        read_message(&mut output)
            .expect("frame")
            .expect("initialize")["id"],
        1
    );
    let diagnostic = read_message(&mut output)
        .expect("frame")
        .expect("diagnostics");
    assert_eq!(diagnostic["method"], "textDocument/publishDiagnostics");
    assert!(
        !diagnostic["params"]["diagnostics"]
            .as_array()
            .expect("diagnostic list")
            .is_empty()
    );
    assert_eq!(
        read_message(&mut output).expect("frame").expect("shutdown")["id"],
        2
    );
    assert!(read_message(&mut output).expect("EOF").is_none());
}

fn read_message(reader: &mut impl std::io::BufRead) -> std::io::Result<Option<serde_json::Value>> {
    lsp_server::Message::read(reader)?
        .map(serde_json::to_value)
        .transpose()
        .map_err(std::io::Error::other)
}
