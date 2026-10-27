//! Print the extracted text of files (manual check of extractors).
//! `cargo run -p tidy-core --example extract -- <file>...`
fn main() {
    let x = tidy_core::index::Extractors::default_set();
    for a in std::env::args().skip(1) {
        let p = std::path::Path::new(&a);
        match x.extract(p) {
            Some(Ok(t)) => println!(
                "== {a} ({} chars)\n{}\n",
                t.chars().count(),
                t.chars().take(300).collect::<String>()
            ),
            Some(Err(e)) => println!("== {a}: ERROR {e}\n"),
            None => println!("== {a}: no extractor\n"),
        }
    }
}
