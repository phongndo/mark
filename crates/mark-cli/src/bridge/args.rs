use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Args, Default)]
pub(crate) struct McpArgs {
    #[arg(long)]
    pub harness: Option<String>,
    #[arg(long)]
    pub conversation: Option<String>,
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum BridgeCommand {
    #[command(about = "Create or resume this harness conversation's review")]
    Open {
        #[arg(long)]
        harness: String,
        #[arg(long)]
        conversation: String,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Call the shared review interface with JSON parameters")]
    Call {
        method: String,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Review in this terminal and return a structured result")]
    Review {
        review_id: String,
        #[arg(long)]
        result_file: PathBuf,
    },
    #[command(about = "Start a local review viewer and print its URL")]
    Web {
        review_id: String,
        #[arg(long)]
        json: bool,
    },
    #[command(hide = true)]
    WebServe {
        review_id: String,
        #[arg(long)]
        ready_file: PathBuf,
    },
}
