use clap::{Parser, ValueEnum};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_CONTEXT_SIZE: u64 = 200_000;

/// Model context window sizes
fn model_context_size(model: &str) -> Option<u64> {
    let model = model.to_lowercase();
    if model.contains("claude-3-5") || model.contains("claude-3.5") {
        Some(200_000)
    } else if model.contains("claude-3") {
        Some(200_000)
    } else if model.contains("claude-sonnet-4") || model.contains("claude-opus-4") || model.contains("claude-haiku-4") {
        Some(200_000)
    } else {
        None
    }
}

#[derive(Parser)]
#[command(name = "claude-ctx", about = "Inspect Claude Code context window usage for a project")]
struct Cli {
    /// Project directory to inspect (defaults to CWD)
    #[arg(long, short)]
    path: Option<PathBuf>,

    /// Output mode
    #[arg(long, short, value_enum, default_value = "bar")]
    output: OutputMode,

    /// Override context window size (tokens). Overrides model detection.
    #[arg(long)]
    context_size: Option<u64>,

    /// Suppress all errors; output nothing on failure
    #[arg(long, short)]
    quiet: bool,
}

#[derive(ValueEnum, Clone)]
enum OutputMode {
    /// Colored block bar + percentage (default)
    Bar,
    /// Raw token count
    Tokens,
    /// Percentage (0-100)
    Percent,
}

#[derive(Deserialize, Debug)]
struct TranscriptEntry {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize, Debug)]
struct Message {
    usage: Option<Usage>,
    model: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Usage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

fn encode_path(path: &Path) -> String {
    // Claude Code encodes project paths by replacing / with - (leading slash becomes leading -)
    let s = path.to_string_lossy();
    s.replace('/', "-")
}

fn find_transcript(project_path: &Path) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let encoded = encode_path(project_path);
    let projects_dir = PathBuf::from(&home).join(".claude/projects").join(&encoded);

    if !projects_dir.exists() {
        return None;
    }

    // Find most recently modified .jsonl file
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    for entry in fs::read_dir(&projects_dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            if let Ok(meta) = fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    match &best {
                        None => best = Some((path, modified)),
                        Some((_, best_time)) if modified > *best_time => {
                            best = Some((path, modified))
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    best.map(|(p, _)| p)
}

struct ContextInfo {
    used_tokens: u64,
    context_size: u64,
}

fn read_transcript(transcript: &Path, context_size_override: Option<u64>) -> Option<ContextInfo> {
    let content = fs::read_to_string(transcript).ok()?;
    let mut used_tokens: u64 = 0;
    let mut detected_model: Option<String> = None;

    // Scan from end for last assistant message with usage
    for line in content.trim().lines().rev() {
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
            if entry.entry_type.as_deref() == Some("assistant") {
                if let Some(msg) = &entry.message {
                    if let Some(usage) = &msg.usage {
                        used_tokens = usage.input_tokens.unwrap_or(0)
                            + usage.cache_creation_input_tokens.unwrap_or(0)
                            + usage.cache_read_input_tokens.unwrap_or(0);

                        if detected_model.is_none() {
                            detected_model = msg.model.clone();
                        }

                        break;
                    }
                }
            }
        }
    }

    let context_size = context_size_override
        .or_else(|| detected_model.as_deref().and_then(model_context_size))
        .unwrap_or(DEFAULT_CONTEXT_SIZE);

    Some(ContextInfo { used_tokens, context_size })
}

fn render_bar(pct: u64) -> String {
    let filled = ((pct as f64 / 100.0) * 10.0).round() as usize;
    let filled = filled.min(10);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));

    let color = if pct >= 85 {
        "\x1b[31m" // red
    } else if pct >= 70 {
        "\x1b[33m" // yellow
    } else {
        "\x1b[32m" // green
    };
    let reset = "\x1b[0m";

    format!("{}{}{} {}%", color, bar, reset, pct)
}

fn main() {
    let cli = Cli::parse();

    let project_path = cli.path.unwrap_or_else(|| {
        std::env::current_dir().expect("cannot determine CWD")
    });

    let transcript = match find_transcript(&project_path) {
        Some(t) => t,
        None => {
            if !cli.quiet {
                eprintln!("claude-ctx: no transcript found for {}", project_path.display());
            }
            std::process::exit(1);
        }
    };

    let info = match read_transcript(&transcript, cli.context_size) {
        Some(i) => i,
        None => {
            if !cli.quiet {
                eprintln!("claude-ctx: failed to read transcript");
            }
            std::process::exit(1);
        }
    };

    let pct = ((info.used_tokens as f64 / info.context_size as f64) * 100.0).round() as u64;
    let pct = pct.min(100);

    match cli.output {
        OutputMode::Tokens => println!("{}", info.used_tokens),
        OutputMode::Percent => println!("{}", pct),
        OutputMode::Bar => println!("{}", render_bar(pct)),
    }
}
