fn main() {
    if let Err(error) = hiraku_lsp::serve_stdio() {
        eprintln!("hiraku-lsp: {error}");
        std::process::exit(1);
    }
}
