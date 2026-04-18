use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "specdev-analyzer",
    version,
    about = "Domain-agnostic analyzer for TOML specs (scaffolding stage)."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate spec files against their JSON Schema and run graph checks.
    Validate {
        /// Paths to spec files or directories.
        paths: Vec<String>,
    },
    /// Emit canonical YAML extraction for codegen.
    Extract {
        /// Output directory.
        #[arg(short, long)]
        output: String,
        /// Paths to spec files or directories.
        paths: Vec<String>,
    },
    /// Compare extracted specs against source code for drift.
    Audit {
        /// Source directory.
        #[arg(long)]
        source: String,
        /// Extracted YAML directory.
        #[arg(long)]
        extracted: String,
    },
    /// Print the resolved context (domain, language, schema paths) for a project.
    Resolve {
        /// Project root containing spec.toml. Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Validate { paths } => {
            eprintln!("specdev-analyzer validate — not yet implemented (Phase 4).");
            eprintln!("received {} path(s)", paths.len());
        }
        Command::Extract { output, paths } => {
            eprintln!("specdev-analyzer extract — not yet implemented (Phase 5).");
            eprintln!("output={output}, paths={}", paths.len());
        }
        Command::Audit { source, extracted } => {
            eprintln!("specdev-analyzer audit — not yet implemented (Phase 8).");
            eprintln!("source={source}, extracted={extracted}");
        }
        Command::Resolve { project } => {
            eprintln!("specdev-analyzer resolve — not yet implemented (Phase 7).");
            eprintln!("project={project}");
        }
    }
    Ok(())
}
