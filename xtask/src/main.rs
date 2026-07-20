use std::{
    path::Path,
    process::{Command, ExitCode},
};

const BOLD_CYAN: &str = "\x1b[1;36m";
const BOLD_GREEN: &str = "\x1b[1;32m";
const BOLD_RED: &str = "\x1b[1;31m";
const RESET: &str = "\x1b[0m";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(task) = args.next() else {
        return usage();
    };

    if task == "migration" {
        return migration(args.collect());
    }

    let ok = match task.as_str() {
        "gen-types" => gen_types(),
        "build-ui" => gen_types() && build_ui(),
        "build" => {
            gen_types() && build_ui() && run("cargo", &["build"], &args.collect::<Vec<_>>(), None)
        }
        _ => return usage(),
    };

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: cargo xtask <gen-types|build-ui|build|migration> [args...]");
    eprintln!("  gen-types       regenerate oxde-ui/src/lib/generated from #[ts(export)] types");
    eprintln!(
        "  build-ui        gen-types, then build oxde-ui/dist via Vite+ (`vp install && vp build`)"
    );
    eprintln!("  build [args]    build-ui, then `cargo build [args]`");
    eprintln!(
        "  migration <generate --name X|apply|drop|reset|snapshot>   database schema migrations (oxde-db/toasty-cli)"
    );
    ExitCode::FAILURE
}

/// Runs a `toasty-cli` migration subcommand against the database at
/// `./data/oxde.db` - the same file `oxde` itself opens by default (see
/// `oxde.toml`'s `data_dir`). `generate` only reads the schema compiled
/// from `oxde_db::models`, so it's safe to run against that file even
/// though it's also the real data `OxDe` serves from.
fn migration(args: Vec<String>) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("failed to start tokio runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run_migration_cli(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_migration_cli(args: Vec<String>) -> anyhow::Result<()> {
    let data_dir = Path::new("data");
    std::fs::create_dir_all(data_dir)?;
    let db = oxde_db::connect(data_dir).await?;

    let cli_args = ["oxde".to_string(), "migration".to_string()]
        .into_iter()
        .chain(args);
    toasty_cli::ToastyCli::new(db).parse_from(cli_args).await
}

fn gen_types() -> bool {
    run("cargo", &["test", "--quiet", "export_bindings"], &[], None)
}

fn build_ui() -> bool {
    let ui_dir = Path::new("oxde-ui");
    run("vp", &["install"], &[], Some(ui_dir)) && run("vp", &["build"], &[], Some(ui_dir))
}

fn run(program: &str, fixed_args: &[&str], extra_args: &[String], dir: Option<&Path>) -> bool {
    let mut command = Command::new(program);
    command.args(fixed_args).args(extra_args);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }

    let where_ = dir.map_or_else(|| ".".to_string(), |dir| dir.display().to_string());
    let all_args = fixed_args
        .iter()
        .copied()
        .chain(extra_args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");

    eprintln!("{BOLD_CYAN}▶ {program} {all_args} (in {where_}){RESET}");

    let result = match command.status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("`{program}` in {where_} exited with {status}");
            false
        }
        Err(err) => {
            eprintln!("failed to run `{program}` in {where_}: {err}");
            false
        }
    };

    let (color, verdict) = if result {
        (BOLD_GREEN, "done")
    } else {
        (BOLD_RED, "failed")
    };
    eprintln!("{color}■ {program} {all_args} - {verdict}{RESET}");
    eprintln!();

    result
}
