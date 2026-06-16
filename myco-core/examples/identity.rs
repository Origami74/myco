//! Smoke check: build the runtime in a temp dir and print the state JSON.
//! `cargo run -p myco-core --example identity`

fn main() {
    let dir = std::env::temp_dir().join("myco-example-identity");
    let rt = myco_core::AppRuntime::new(dir.to_str().unwrap(), "0.0.1");
    println!("{}", rt.state_json());
}
