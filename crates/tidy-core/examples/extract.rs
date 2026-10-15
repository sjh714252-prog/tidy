//! Debug helper: `cargo run -p tidy-core --example extract -- <file>...`
//! prints the text an extractor produces for each file.
use tidy_core::index::Extractors;

fn main() {
    let x = Extractors::default_set();
    for arg in std::env::args().skip(1) {
        println!("=== {arg}");
        match x.extract(std::path::Path::new(&arg)) {
            None => println!("(unsupported)"),
            Some(Ok(t)) => println!("{t}"),
            Some(Err(e)) => println!("ERROR: {e}"),
        }
    }
}
