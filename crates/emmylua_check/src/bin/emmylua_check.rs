use clap::Parser;
use emmylua_check::{CheckFailed, OutputFormat, cmd_args::CmdArgs, run_check};
use mimalloc::MiMalloc;
use std::error::Error;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    let cmd_args = CmdArgs::parse();
    let compact = cmd_args.output_format == OutputFormat::Gcc;
    let result = run_check(cmd_args).await;
    if compact
        && let Err(error) = &result
        && let Some(failed) = error.downcast_ref::<CheckFailed>()
    {
        std::process::exit(failed.0);
    }
    result
}
