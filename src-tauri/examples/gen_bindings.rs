fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/ipc/bindings.ts");
    match onto_studio_lib::gen_bindings(path) {
        Ok(()) => println!("[onto-studio] bindings exported to {path}"),
        Err(e) => { eprintln!("[onto-studio] ERROR: {e}"); std::process::exit(1); }
    }
}
