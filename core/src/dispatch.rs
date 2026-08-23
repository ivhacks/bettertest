use crate::pipedef::*;
use serde_json::*;
use std::path::*;

pub fn entry(pipedef: PathBuf) {
    let parsed = parse(&pipedef);
    println!("{}", to_string_pretty(&parsed).unwrap());
}
