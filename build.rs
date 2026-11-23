#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "src/canon.rs"]
mod canon;

fn main() {
    println!("cargo:rerun-if-changed=canon");
    println!("cargo:rerun-if-changed=src/canon.rs");
    let canon = canon::Canon::load(&canon::CanonSources::embedded())
        .unwrap_or_else(|error| panic!("canon refused: {error}"));
    let bytes = serde_json::to_vec(&canon).expect("serialize validated canon");
    let out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    std::fs::write(out.join("canon.json"), bytes).expect("write compiled canon");
}
