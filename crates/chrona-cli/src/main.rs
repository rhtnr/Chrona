mod commands;
mod wav;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "chrona", version, about = "Chrona timegrapher CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a synthetic watch recording (deterministic ground truth)
    Synth(commands::SynthArgs),
    /// Analyze a WAV recording of a mechanical watch
    Analyze(commands::AnalyzeArgs),
    /// Run expectation files against their recordings (regression corpus)
    Verify(commands::VerifyArgs),
}

fn main() {
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run(cli: Cli) -> anyhow::Result<i32> {
    match cli.cmd {
        Cmd::Synth(a) => {
            commands::run_synth(&a)?;
            Ok(0)
        }
        Cmd::Analyze(a) => {
            let report = commands::run_analyze(&a)?;
            if a.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                commands::print_human(&report);
            }
            Ok(if report.status == "ok" { 0 } else { 2 })
        }
        Cmd::Verify(a) => {
            let failures = commands::run_verify(&a)?;
            Ok(if failures == 0 { 0 } else { 1 })
        }
    }
}
