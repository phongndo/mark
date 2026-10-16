use crate::{
    CliResult,
    args::{self, Cli, Command},
    config,
    pager::pager,
    review::{
        ReviewRequest, compare_options, diff_options, difftool_options, patch_options,
        review_options, run_review, show_options,
    },
    session::session,
    skill::skill,
    syntax::syntax,
    update::update,
};

pub(crate) fn run_cli(cli: Cli) -> CliResult<()> {
    match cli.command {
        None => Ok(()),
        Some(Command::Config) => config::config(),
        Some(Command::Diff(args)) => {
            run_review_command(&args.display, args.watch.watch, diff_options(&args))
        }
        Some(Command::Compare(args)) => {
            run_review_command(&args.display, args.watch.watch, compare_options(&args)?)
        }
        Some(Command::Difftool(args)) => {
            run_review_command(&args.display, args.watch.watch, difftool_options(&args))
        }
        Some(Command::Pager(args)) => pager(args),
        Some(Command::Show(args)) => run_review_command(&args.display, false, show_options(&args)),
        Some(Command::Review(args)) => {
            run_review_command(&args.display, false, review_options(&args)?)
        }
        Some(Command::Patch(args)) => {
            run_review_command(&args.display, false, patch_options(&args)?)
        }
        Some(Command::Session { command }) => session(command),
        Some(Command::Skill { command }) => skill(command),
        Some(Command::Syntax { command }) => syntax(command),
        Some(Command::Update(args)) => update(args),
    }
}

fn run_review_command(
    display: &args::DisplayArgs,
    live_updates: bool,
    options: mark_command::DiffOptions,
) -> CliResult<()> {
    run_review(ReviewRequest {
        options,
        live_updates,
        syntax_enabled: display.syntax_enabled(),
        empty_diff_fill: display.empty_diff_fill_override(),
        decorations: display.decoration_override(),
    })
}
