//! The SCIP indexers `index` runs (`specs/02-extraction.md` § Acquisition).
//!
//! The only code in this binary that executes another program, and it runs
//! exactly one thing: the `Lang.indexer` of a language the walk found. Never a
//! build, a test, a task runner or an install. Nothing here matches on a
//! language's name; every decision reads the row.
//!
//! Never called on the query path: `query` and `mcp` re-read the outputs the
//! manifest names and run nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use extract::lang::Indexer;
use facts::IndexerRun;

/// Wall clock per program. The child is killed and the run reported after it.
pub const TIMEOUT: Duration = Duration::from_secs(600);

/// The store subdirectory that holds the outputs and their logs. `--rebuild`'s
/// discard keeps it (`specs/04-storage.md` § Layout).
pub const DIR: &str = "scip";

/// How often a running child is polled for exit.
const POLL: Duration = Duration::from_millis(100);

/// A program to consider in one root: its row, and the newest mtime among
/// the files of its languages under that root.
struct Job {
    indexer: Indexer,
    /// Repo-relative, empty for the repository root.
    root: String,
    newest: u64,
    /// False when `--lang` left out every language it serves: its output is
    /// still read, it is never run.
    wanted: bool,
    /// No directory in the tree holds the row's `needs` file.
    root_missing: bool,
}

/// Run every indexer the tree needs and is stale, once per indexer root, into
/// `<store>/scip/`, and report each to stderr as it goes.
///
/// Returns one record per program and root considered. A program `--lang` left out, or
/// one that is up to date, keeps its `prior` record. A failure is an outcome,
/// never an `Err`: tier A stands whatever an indexer does.
///
/// # Errors
/// Creating `<store>/scip/` fails.
pub fn run(
    root: &Path,
    store: &Path,
    langs: &[String],
    rebuild: bool,
    prior: &[IndexerRun],
) -> Result<Vec<IndexerRun>> {
    let jobs = jobs(root, langs);
    if jobs.is_empty() {
        return Ok(Vec::new());
    }
    let dir = store.join(DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut runs = Vec::new();
    for job in jobs.values() {
        let program = job.indexer.program;
        let previous = prior
            .iter()
            .find(|r| r.program == program && r.root == job.root);
        let at = dir.join(&job.root);
        let output = at.join(format!("{program}.scip"));
        let log = at.join(format!("{program}.log"));
        let label = if job.root.is_empty() {
            program.to_string()
        } else {
            format!("{program} in {}", job.root)
        };
        let mut record = IndexerRun {
            program: program.to_string(),
            command: job.indexer.to_string(),
            root: job.root.clone(),
            outcome: String::new(),
            output: output.display().to_string(),
            log: String::new(),
        };

        if !job.wanted {
            if let Some(previous) = previous {
                runs.push(previous.clone());
            }
            continue;
        }
        if job.root_missing {
            let needs = job.indexer.needs.unwrap_or_default();
            record.outcome = format!("no {needs} in the tree");
            eprintln!("  {program}: skipped — {}", record.outcome);
            runs.push(record);
            continue;
        }
        let Some(binary) = on_path(program) else {
            record.outcome = "not on PATH".to_string();
            eprintln!("  {label}: skipped — not on PATH; install it, then re-run");
            runs.push(record);
            continue;
        };
        if !rebuild
            && let Some(previous) = previous
            && previous.ok()
            && mtime(&output).is_some_and(|built| built >= job.newest)
        {
            eprintln!("  {label}: up to date");
            runs.push(previous.clone());
            continue;
        }

        std::fs::create_dir_all(&at).with_context(|| format!("creating {}", at.display()))?;
        record.log = log.display().to_string();
        eprintln!(
            "codeintel: running {} {} {} (log: {})",
            record.command,
            job.indexer.output,
            output.display(),
            log.display()
        );
        let started = Instant::now();
        record.outcome = execute(&root.join(&job.root), &binary, &job.indexer, &output, &log);
        let secs = started.elapsed().as_secs_f64();
        if record.ok() {
            eprintln!("  {label}: ok in {secs:.1} s");
        } else {
            eprintln!("  {label}: {}; see {}", record.outcome, log.display());
        }
        runs.push(record);
    }
    Ok(runs)
}

/// The programs the tree needs, one per indexer root, keyed by program then
/// root so two languages sharing a program run it once per root.
///
/// A program whose `needs` file is nowhere in the tree gets one job at the
/// repository root with `root_missing` set, so the skip is recorded.
fn jobs(root: &Path, langs: &[String]) -> BTreeMap<(&'static str, String), Job> {
    let walked = extract::walk(root);
    let mut roots_of: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let mut jobs: BTreeMap<(&'static str, String), Job> = BTreeMap::new();
    for file in &walked.files {
        let indexer = file.lang.indexer;
        let wanted = langs.is_empty() || langs.iter().any(|l| l == file.lang.name);
        let roots = roots_of
            .entry(indexer.program)
            .or_insert_with(|| roots(indexer.needs, &walked.visible));
        let (at, root_missing) = match roots.iter().find(|r| under(&file.path, r)) {
            Some(at) => (at.clone(), false),
            // Outside every root: no run covers it. Recorded only when there
            // is no root at all, which is the skip worth reporting.
            None if roots.is_empty() => (String::new(), true),
            None => continue,
        };
        let job = jobs.entry((indexer.program, at.clone())).or_insert(Job {
            indexer,
            root: at,
            root_missing,
            newest: 0,
            wanted: false,
        });
        job.newest = job.newest.max(file.mtime);
        job.wanted |= wanted;
    }
    jobs
}

/// The outermost directories holding `needs`, repo-relative, empty for the
/// repository root. A row with no `needs` has the repository root alone.
/// The file is checked to exist, never read.
fn roots(needs: Option<&str>, visible: &BTreeSet<String>) -> Vec<String> {
    let Some(needs) = needs else {
        return vec![String::new()];
    };
    let holding: Vec<&str> = visible
        .iter()
        .filter_map(|path| match path.strip_suffix(needs)? {
            "" => Some(""),
            dir => dir.strip_suffix('/'),
        })
        .collect();
    holding
        .iter()
        .filter(|dir| {
            !holding
                .iter()
                .any(|other| other != *dir && under(dir, other))
        })
        .map(|dir| (*dir).to_string())
        .collect()
}

/// `path` is `dir` or below it. Everything is below the repository root.
fn under(path: &str, dir: &str) -> bool {
    dir.is_empty()
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Run one indexer into a temporary file and rename it into place on success,
/// so no reader ever ingests half an output. Returns the outcome.
fn execute(root: &Path, binary: &Path, indexer: &Indexer, output: &Path, log: &Path) -> String {
    let tmp = output.with_extension(format!("scip.tmp-{}", std::process::id()));
    let outcome = spawn_and_wait(root, binary, indexer, &tmp, log);
    let outcome = match outcome {
        Ok(outcome) if outcome == "ok" => {
            if std::fs::metadata(&tmp).is_ok_and(|m| m.len() > 0) {
                match std::fs::rename(&tmp, output) {
                    Ok(()) => "ok".to_string(),
                    Err(e) => format!("could not keep its output: {e}"),
                }
            } else {
                "no output".to_string()
            }
        }
        Ok(outcome) => outcome,
        Err(e) => format!("could not start: {e:#}"),
    };
    // Gone on success (renamed); a failure's partial output is never read.
    drop(std::fs::remove_file(&tmp));
    outcome
}

/// `ok`, `exit <code>`, `killed by a signal`, or `timed out after <n> s`.
fn spawn_and_wait(
    root: &Path,
    binary: &Path,
    indexer: &Indexer,
    tmp: &Path,
    log: &Path,
) -> Result<String> {
    let out = File::create(log).with_context(|| format!("creating {}", log.display()))?;
    let err = out.try_clone().context("sharing the log")?;
    let mut child = Command::new(binary)
        .args(indexer.args)
        .arg(indexer.output)
        .arg(tmp)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .with_context(|| format!("spawning {}", binary.display()))?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().context("waiting on the indexer")? {
            return Ok(if status.success() {
                "ok".to_string()
            } else {
                status
                    .code()
                    .map_or_else(|| "killed by a signal".to_string(), |c| format!("exit {c}"))
            });
        }
        if Instant::now() >= deadline {
            // Only the direct child: an indexer that forked keeps its children
            // until they notice. Reported, never silent (invariant 5).
            drop(child.kill());
            drop(child.wait());
            return Ok(format!("timed out after {} s", TIMEOUT.as_secs()));
        }
        std::thread::sleep(POLL);
    }
}

/// `program`'s path, from `PATH`. A lookup, never an install.
///
/// Absolute entries only. An empty or relative entry means the working
/// directory, which is the repository, and a file the repository names
/// `rust-analyzer` is not the indexer.
fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Seconds since the epoch, as the walk reports a file's.
fn mtime(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}
