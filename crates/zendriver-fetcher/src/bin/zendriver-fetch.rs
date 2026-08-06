//! `zendriver-fetch` — download a Chromium build from the command line.
//!
//! Two modes, and which one runs is decided by what you passed:
//!
//! **Non-interactive.** Give a distribution and a version (or revision) and
//! the tool never prompts, never blocks on stdin, and exits non-zero on any
//! failure:
//!
//! ```text
//! zendriver-fetch --distribution cft --version 146.0.7680.153 \
//!                 --platform mac-arm64 --out ./chrome
//! ```
//!
//! **Interactive.** With those missing, it asks — distribution first, then a
//! paged menu of the builds that distribution *actually publishes for the
//! resolved platform*, then a confirmation before anything downloads.
//!
//! The two modes meet at a guard rail: if stdin is not a terminal, the
//! interactive path **refuses to prompt** and instead prints the flags it
//! needed. A CLI that blocks on stdin inside CI does not fail — it hangs
//! until the job times out, which is the worst outcome for a tool built to
//! run unattended.
//!
//! The menu is hand-rolled rather than pulled from a prompt crate: it is a
//! numbered list and a line of stdin, and keeping it in-tree keeps a
//! dependency out of the graph of a crate whose whole job is to be a small
//! library.

use std::io::{IsTerminal as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use zendriver_fetcher::{
    Build, Distribution, Fetcher, FetcherPhase, FetcherProgress, Platform, VersionSpec, list_builds,
};

/// Builds shown per page in the interactive picker. Chrome for Testing lists
/// thousands of versions; a page has to fit on a terminal.
const PAGE: usize = 15;

#[derive(Debug, Parser)]
#[command(
    name = "zendriver-fetch",
    about = "Download a Chromium build (Chrome for Testing, ungoogled-chromium, or a Chromium snapshot)",
    // No `version` attribute: `--version` selects the *browser* version here,
    // which is the whole point of the tool.
    disable_version_flag = true
)]
struct Args {
    /// Which build to fetch: cft, ungoogled, or snapshot.
    #[arg(short, long, value_name = "NAME")]
    distribution: Option<String>,

    /// Browser version, e.g. 146.0.7680.153, or `latest`.
    ///
    /// Matched exactly for Chrome for Testing and as a tag prefix for
    /// ungoogled-chromium. Not accepted for snapshots, which are keyed by
    /// revision — use --revision there.
    #[arg(long, value_name = "VERSION")]
    version: Option<String>,

    /// Chromium revision, e.g. 1674890. Snapshots only.
    #[arg(long, value_name = "N", conflicts_with = "version")]
    revision: Option<u64>,

    /// Target platform: linux64, mac-x64, mac-arm64, win32, win64.
    /// Defaults to this host.
    #[arg(short, long, value_name = "PLATFORM")]
    platform: Option<String>,

    /// Directory to cache the build under. Defaults to the OS cache dir.
    #[arg(short, long, value_name = "DIR")]
    out: Option<PathBuf>,

    /// Suppress progress output. Errors still go to stderr.
    #[arg(short, long)]
    quiet: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let quiet = args.quiet;
    match run(args).await {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            // Progress is a single rewritten line with no trailing newline,
            // so close it before the error rather than running into it.
            if !quiet {
                eprintln!();
            }
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<PathBuf, String> {
    let platform = match &args.platform {
        Some(name) => Platform::parse(name).ok_or_else(|| {
            format!(
                "unknown platform {name:?}; expected one of: {}",
                Platform::ALL
                    .iter()
                    .map(|p| p.as_cft_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?,
        None => Platform::auto_detect()
            .ok_or("this host is not a supported platform; pass --platform explicitly")?,
    };

    let (distribution, spec) = select_build(&args, platform).await?;

    let quiet = args.quiet;
    if !quiet {
        eprintln!(
            "fetching {} for {} ...",
            distribution.title(),
            platform.as_cft_str()
        );
    }

    let mut fetcher = Fetcher::new()
        .distribution(distribution)
        .platform(platform)
        .version(spec);
    if let Some(dir) = args.out {
        fetcher = fetcher.cache_dir(dir);
    }
    if !quiet {
        fetcher = fetcher.on_progress(report_progress);
    }

    let path = fetcher.ensure_chrome().await.map_err(|e| e.to_string())?;
    if !quiet {
        eprintln!();
    }
    Ok(path)
}

/// Work out which build to fetch, prompting only when a flag is missing *and*
/// there is a human on the other end of stdin.
async fn select_build(
    args: &Args,
    platform: Platform,
) -> Result<(Distribution, VersionSpec), String> {
    let distribution = match &args.distribution {
        Some(name) => Some(Distribution::parse(name).ok_or_else(|| {
            format!(
                "unknown distribution {name:?}; expected one of: {}",
                Distribution::ALL
                    .iter()
                    .map(|d| d.slug())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?),
        None => None,
    };

    let spec = match (&args.version, args.revision) {
        (Some(v), _) if v.eq_ignore_ascii_case("latest") => Some(VersionSpec::Latest),
        (Some(v), _) => Some(VersionSpec::Explicit(v.clone())),
        (None, Some(rev)) => Some(VersionSpec::Revision(rev)),
        (None, None) => None,
    };

    // Fully specified: no prompting, whatever the terminal looks like.
    if let (Some(distribution), Some(spec)) = (distribution, spec.clone()) {
        return Ok((distribution, spec));
    }

    if !std::io::stdin().is_terminal() {
        return Err(missing_flags_message(
            distribution.is_none(),
            spec.is_none(),
        ));
    }

    let distribution = match distribution {
        Some(d) => d,
        None => prompt_distribution()?,
    };
    let spec = match spec {
        Some(s) => s,
        None => prompt_build(distribution, platform).await?,
    };
    Ok((distribution, spec))
}

/// The message a non-interactive caller gets instead of a hung prompt.
fn missing_flags_message(needs_distribution: bool, needs_version: bool) -> String {
    let mut missing = Vec::new();
    if needs_distribution {
        missing.push("--distribution <cft|ungoogled|snapshot>");
    }
    if needs_version {
        missing.push("--version <VERSION|latest> (or --revision <N> for snapshots)");
    }
    format!(
        "stdin is not a terminal, so there is nobody to prompt — pass {} on the command line",
        missing.join(" and ")
    )
}

fn prompt_distribution() -> Result<Distribution, String> {
    eprintln!("Which distribution?");
    for (i, d) in Distribution::ALL.iter().enumerate() {
        eprintln!("  {}) {:<20} [{}]", i + 1, d.title(), d.slug());
    }
    loop {
        let line = read_line("distribution [1]: ")?;
        let line = line.trim();
        if line.is_empty() {
            // Whatever `1)` printed above — reordering the menu must not leave
            // the default pointing somewhere else.
            return Ok(Distribution::ALL[0]);
        }
        if let Some(d) = line
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=Distribution::ALL.len()).contains(n))
            .map(|n| Distribution::ALL[n - 1])
            .or_else(|| Distribution::parse(line))
        {
            return Ok(d);
        }
        eprintln!("  not one of the options — enter a number or a slug");
    }
}

/// Page through the builds this distribution really has for this platform.
async fn prompt_build(
    distribution: Distribution,
    platform: Platform,
) -> Result<VersionSpec, String> {
    eprintln!(
        "Listing {} builds for {} ...",
        distribution.title(),
        platform.as_cft_str()
    );
    let builds = list_builds(distribution, platform)
        .await
        .map_err(|e| e.to_string())?;

    if builds.is_empty() {
        return Err(format!(
            "{} publishes no builds for {}",
            distribution.title(),
            platform.as_cft_str()
        ));
    }

    let mut page = 0usize;
    loop {
        let start = page * PAGE;
        let shown = &builds[start..builds.len().min(start + PAGE)];
        for (offset, build) in shown.iter().enumerate() {
            eprintln!("  {:>3}) {}", start + offset + 1, build.label);
        }

        let has_more = start + PAGE < builds.len();
        let prompt = if has_more {
            format!(
                "number, `m` for more ({} of {} shown), or `q`: ",
                start + shown.len(),
                builds.len()
            )
        } else {
            "number, or `q`: ".to_string()
        };

        let line = read_line(&prompt)?;
        let line = line.trim();
        match line {
            "q" | "Q" => return Err("cancelled".to_string()),
            "m" | "M" if has_more => {
                page += 1;
                continue;
            }
            _ => {}
        }

        let Some(build) = line
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=builds.len()).contains(n))
            .map(|n| &builds[n - 1])
        else {
            eprintln!("  enter a listed number");
            continue;
        };

        if confirm(build, platform)? {
            return Ok(build.spec.clone());
        }
    }
}

fn confirm(build: &Build, platform: Platform) -> Result<bool, String> {
    let answer = read_line(&format!(
        "Download {} for {}? [y/N]: ",
        build.label,
        platform.as_cft_str()
    ))?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES"))
}

fn read_line(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    std::io::stderr()
        .flush()
        .map_err(|e| format!("stderr: {e}"))?;
    let mut buf = String::new();
    match std::io::stdin().read_line(&mut buf) {
        // EOF: the terminal went away mid-prompt. Bail rather than spin.
        Ok(0) => Err("stdin closed".to_string()),
        Ok(_) => Ok(buf),
        Err(e) => Err(format!("stdin: {e}")),
    }
}

/// One rewritten line on stderr, so piping stdout still yields just the path.
fn report_progress(p: FetcherProgress) {
    let line = match (p.phase, p.total) {
        (FetcherPhase::Downloading, Some(total)) if total > 0 => {
            let pct = (p.downloaded as f64 / total as f64) * 100.0;
            format!(
                "downloading {:>5.1}%  ({:.1}/{:.1} MiB)",
                pct,
                mib(p.downloaded),
                mib(total)
            )
        }
        (FetcherPhase::Downloading, _) => {
            format!("downloading {:.1} MiB", mib(p.downloaded))
        }
        (FetcherPhase::Resolving, _) => "resolving".to_string(),
        (FetcherPhase::Extracting, _) => "unpacking".to_string(),
        (FetcherPhase::Verifying, _) => "verifying".to_string(),
        (FetcherPhase::Done, _) => "done".to_string(),
    };
    eprint!("\r\x1b[2K{line}");
    let _ = std::io::stderr().flush();
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn clap_definition_is_valid() {
        Args::command().debug_assert();
    }

    /// The documented non-interactive invocation must parse into a fully
    /// specified request — nothing left for a prompt to fill in.
    #[test]
    fn the_documented_ci_invocation_needs_no_prompting() {
        let args = Args::parse_from([
            "zendriver-fetch",
            "--distribution",
            "cft",
            "--version",
            "146.0.7680.153",
            "--platform",
            "mac-arm64",
            "--out",
            "/tmp/chrome",
        ]);
        assert_eq!(args.distribution.as_deref(), Some("cft"));
        assert_eq!(args.version.as_deref(), Some("146.0.7680.153"));
        assert_eq!(args.platform.as_deref(), Some("mac-arm64"));
        assert_eq!(args.out, Some(PathBuf::from("/tmp/chrome")));
        assert!(!args.quiet);
    }

    #[test]
    fn version_and_revision_are_mutually_exclusive() {
        let err = Args::try_parse_from([
            "zendriver-fetch",
            "--version",
            "151.0.7922.71",
            "--revision",
            "1674890",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    /// The refusal has to name the flags, or a CI failure tells the operator
    /// nothing actionable.
    #[test]
    fn non_tty_refusal_names_the_missing_flags() {
        let msg = missing_flags_message(true, true);
        assert!(msg.contains("--distribution"), "{msg}");
        assert!(msg.contains("--version"), "{msg}");
        assert!(msg.contains("--revision"), "{msg}");

        let msg = missing_flags_message(false, true);
        assert!(!msg.contains("--distribution"), "{msg}");
        assert!(msg.contains("--version"), "{msg}");
    }
}
