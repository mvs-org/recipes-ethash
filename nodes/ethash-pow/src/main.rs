//! Basic POW Node Template CLI library.
#![warn(missing_docs)]

mod chain_spec;
#[macro_use]
mod service;
mod cli;
mod command;
mod rpc;
mod types;

fn main() -> sc_cli::Result<()> {
	command::run()
}
