mod actions;
mod app;
mod book;
mod config;
mod disguise;
mod progress;
mod render;

use std::{env, path::PathBuf, process::ExitCode};

const USAGE: &str = "\
usage: herdr-reader [FILE]           read FILE (.txt/.epub/.pdf), or the last book
       herdr-reader --action <ID>    herdr plugin action: open|close|toggle|next|prev|boss|info
       herdr-reader --dump FILE      print the extracted paragraphs (check a PDF's text layer)

keys:  j/space/enter/PgDn next page  k/PgUp prev page  J/K ±10 pages  i progress
       o open library  b boss mode  q quit";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let res = match args.as_slice() {
        [flag, id] if flag == "--action" => actions::run(id),
        [flag, file] if flag == "--dump" => dump(file),
        [h] if h == "-h" || h == "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        [] => app::run(None),
        [file] if !file.starts_with('-') => app::run(Some(PathBuf::from(file))),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("reader: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn dump(file: &str) -> anyhow::Result<()> {
    for (i, p) in book::load(std::path::Path::new(file))?.iter().enumerate() {
        println!("{i:5}  {p}");
    }
    Ok(())
}
