mod cli;
mod config;
mod metadata;
mod model;
mod operations;
mod parser;
mod quality;
mod rename;
mod report;
mod scanning;
#[cfg(test)]
mod tests;
mod tmdb;
mod tvmaze;
mod workflow;
#[cfg(test)]
mod workflow_tests;

use clap::Parser as _;

fn main() {
    let args = cli::Args::parse();
    if let Err(error) = workflow::run(args) {
        eprintln!("{} {error:#}", console::style("error:").red().bold());
        std::process::exit(1);
    }
}
