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

    let ok = match task.as_str() {
        "build-ui" => build_ui(),
        "build" => build_ui() && run("cargo", &["build"], &args.collect::<Vec<_>>(), None),
        _ => return usage(),
    };

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: cargo xtask <build-ui|build> [args...]");
    eprintln!("  build-ui        build oxde-ui/dist via Vite+ (`vp install && vp build`)");
    eprintln!("  build [args]    build-ui, then `cargo build [args]`");
    ExitCode::FAILURE
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
