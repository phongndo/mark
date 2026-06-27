use crate::{
    CliResult,
    args::{self, Cli, Command},
    config,
    pager::pager,
    preflight::{reject_likely_unknown_command, reject_pre_subcommand_diff_args},
    review::{ReviewRequest, run_review},
    syntax::{diff_options, difftool_options, patch_options, review_options, show_options, syntax},
    update::update,
};

pub(crate) fn run_cli(cli: Cli) -> CliResult<()> {
    reject_pre_subcommand_diff_args(&cli)?;
    let Cli { command, diff } = cli;
    match command {
        None => {
            reject_likely_unknown_command(&diff)?;
            run_diff(diff)
        }
        Some(Command::Config) => config::config(),
        Some(Command::Diff(args)) => run_diff(args),
        Some(Command::Difftool(args)) => run_difftool(args),
        Some(Command::Pager(args)) => pager(args),
        Some(Command::Show(args)) => run_show(args),
        Some(Command::Review(args)) => run_hosted_review(args),
        Some(Command::Patch(args)) => run_patch(args),
        Some(Command::Syntax { command }) => syntax(command),
        Some(Command::Update(args)) => update(args),
    }
}

fn run_diff(args: args::DiffArgs) -> CliResult<()> {
    let live_updates = !args.no_watch;
    let syntax_enabled = !args.no_syntax;
    let options = diff_options(args)?;
    run_review(ReviewRequest {
        options,
        live_updates,
        syntax_enabled,
    })
}

fn run_show(args: args::ShowArgs) -> CliResult<()> {
    let syntax_enabled = !args.no_syntax;
    let options = show_options(args)?;
    run_review(ReviewRequest {
        options,
        live_updates: false,
        syntax_enabled,
    })
}

fn run_hosted_review(args: args::ReviewArgs) -> CliResult<()> {
    let syntax_enabled = !args.no_syntax;
    let options = review_options(args)?;
    run_review(ReviewRequest {
        options,
        live_updates: false,
        syntax_enabled,
    })
}

fn run_difftool(args: args::DifftoolArgs) -> CliResult<()> {
    let live_updates = args.watch;
    let syntax_enabled = !args.no_syntax;
    let options = difftool_options(args)?;
    run_review(ReviewRequest {
        options,
        live_updates,
        syntax_enabled,
    })
}

fn run_patch(args: args::PatchArgs) -> CliResult<()> {
    let syntax_enabled = !args.no_syntax;
    let options = patch_options(args)?;
    run_review(ReviewRequest {
        options,
        live_updates: false,
        syntax_enabled,
    })
}
