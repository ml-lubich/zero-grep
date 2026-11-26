use clap::Parser;
use ignore::WalkBuilder;
use memmap2::Mmap;
use rayon::prelude::*;
use regex::RegexBuilder;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
mod index;
#[cfg(feature = "python")]
use pyo3::types::PyModuleMethods;

#[derive(Parser)]
#[command(
    version,
    about = "Fast, portable code search for developers and AI agents"
)]
struct Args {
    /// Regular expression to search
    pattern: String,
    /// Files or directories to search
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
    /// Match without regard to case
    #[arg(short, long)]
    ignore_case: bool,
    /// Search hidden files
    #[arg(short = 'H', long)]
    hidden: bool,
    /// Disable .gitignore rules
    #[arg(long)]
    no_ignore: bool,
    /// Number of worker threads (defaults to available CPUs)
    #[arg(short = 'j', long)]
    threads: Option<usize>,
    /// Rebuild the on-disk trigram index before searching
    #[arg(long)]
    index: bool,
    /// Search files directly without the trigram index
    #[arg(long)]
    no_index: bool,
    /// Exit with status only; print no match lines
    #[arg(short, long)]
    quiet: bool,
    /// Print only paths with matches
    #[arg(short = 'l', long)]
    files_with_matches: bool,
}

pub fn cli() -> u8 {
    cli_from(std::env::args_os().skip(1))
}

fn cli_from(args: impl IntoIterator<Item = std::ffi::OsString>) -> u8 {
    let args = Args::parse_from(std::iter::once(std::ffi::OsString::from("zg")).chain(args));
    match run(args) {
        Ok(found) => {
            if found {
                0
            } else {
                1
            }
        }
        Err(e) => {
            eprintln!("zg: {e}");
            2
        }
    }
}

#[cfg(feature = "python")]
#[pyo3::pyfunction]
fn python_main(args: Vec<String>) -> pyo3::PyResult<()> {
    let code = cli_from(args.into_iter().map(std::ffi::OsString::from));
    if code == 0 {
        Ok(())
    } else {
        Err(pyo3::exceptions::PySystemExit::new_err(code))
    }
}

#[cfg(feature = "python")]
#[pyo3::pymodule]
fn _core(module: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
    module.add_function(pyo3::wrap_pyfunction!(python_main, module)?)?;
    Ok(())
}

fn run(args: Args) -> Result<bool, Box<dyn std::error::Error>> {
    if let Some(n) = args.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()?;
    }
    let re = RegexBuilder::new(&args.pattern)
        .case_insensitive(args.ignore_case)
        .build()?;
    let mut files = Vec::new();
    if args.paths.len() == 1 && args.paths[0].is_dir() && !args.no_index {
        let root = &args.paths[0];
        let index = index::SearchIndex::open(root, args.index, args.hidden, args.no_ignore)?;
        let candidates = if args.ignore_case {
            None
        } else {
            index.candidates(&args.pattern)
        };
        let (fresh, mut dirty) = index.fresh_files(root, args.hidden, args.no_ignore);
        if let Some(candidates) = candidates {
            dirty.extend(
                candidates
                    .into_iter()
                    .filter(|p| fresh.contains(p))
                    .map(|p| root.join(p)),
            );
            files = dirty;
        } else {
            files = dirty;
            files.extend(fresh.into_iter().map(|p| root.join(p)));
        }
    } else {
        for root in &args.paths {
            if root.is_file() {
                files.push(root.clone());
                continue;
            }
            let mut walk = WalkBuilder::new(root);
            walk.hidden(!args.hidden)
                .ignore(!args.no_ignore)
                .git_ignore(!args.no_ignore)
                .parents(!args.no_ignore);
            files.extend(
                walk.build()
                    .filter_map(Result::ok)
                    .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                    .map(|e| e.into_path()),
            );
        }
    }
    let mut hits: Vec<(PathBuf, String)> = files
        .par_iter()
        .filter_map(|p| scan(p, &re).map(|s| (p.clone(), s)))
        .collect();
    hits.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let found = !hits.is_empty();
    if !args.quiet {
        for (path, hit) in hits {
            if args.files_with_matches {
                println!("{}", path.display());
            } else {
                print!("{hit}");
            }
        }
    }
    Ok(found)
}

fn scan(path: &Path, re: &regex::Regex) -> Option<String> {
    let file = File::open(path).ok()?;
    // mmap avoids copying regular files; fall back to buffered reads where mapping fails.
    let mapped = unsafe { Mmap::map(&file).ok() };
    let fallback;
    let bytes: &[u8] = if let Some(ref m) = mapped {
        m
    } else {
        let mut data = Vec::new();
        File::open(path).ok()?.read_to_end(&mut data).ok()?;
        fallback = data;
        &fallback
    };
    if bytes.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut out = String::new();
    for (line_number, line) in text.lines().enumerate() {
        if re.is_match(line) {
            out.push_str(&format!(
                "{}:{}:{}\n",
                path.display(),
                line_number + 1,
                line
            ));
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regex_finds_lines_and_skips_binary() {
        let root = std::env::temp_dir().join(format!("zero-grep-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let text = root.join("text.rs");
        std::fs::write(&text, "alpha\nBeta 42\nomega\n").unwrap();
        let bin = root.join("binary");
        std::fs::write(&bin, b"Beta\0skip").unwrap();
        let invalid = root.join("invalid-utf8");
        std::fs::write(&invalid, [0xff]).unwrap();
        let re = RegexBuilder::new(r"beta \d+")
            .case_insensitive(true)
            .build()
            .unwrap();
        assert_eq!(
            scan(&text, &re).unwrap(),
            format!("{}:2:Beta 42\n", text.display())
        );
        assert!(scan(&bin, &re).is_none());
        assert!(scan(&invalid, &re).is_none());
        let re = RegexBuilder::new("a|e").build().unwrap();
        assert_eq!(scan(&text, &re).unwrap().lines().count(), 3);
        let multiline = root.join("multiline.txt");
        std::fs::write(&multiline, "static\nint fn_probe()\n").unwrap();
        let re = RegexBuilder::new(r"static\s+int\s+\w+_probe\(")
            .build()
            .unwrap();
        assert!(scan(&multiline, &re).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scan_falls_back_when_mmap_is_unavailable() {
        let re = RegexBuilder::new("anything").build().unwrap();
        assert!(scan(Path::new("/dev/null"), &re).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn scan_falls_back_when_windows_device_cannot_be_mapped() {
        let re = RegexBuilder::new("anything").build().unwrap();
        assert!(scan(Path::new("NUL"), &re).is_none());
    }

    #[test]
    fn run_searches_direct_file_and_returns_no_match() {
        let root = std::env::temp_dir().join(format!("zero-grep-run-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("one.txt");
        std::fs::write(&file, "alpha\n").unwrap();
        let args = Args {
            pattern: "missing".into(),
            paths: vec![file],
            ignore_case: false,
            hidden: false,
            no_ignore: false,
            threads: Some(1),
            index: false,
            no_index: true,
            quiet: false,
            files_with_matches: false,
        };
        assert!(!run(args).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_walks_directory_and_matches_regex() {
        let root = std::env::temp_dir().join(format!("zero-grep-walk-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/a.txt"), "x42\n").unwrap();
        let args = Args {
            pattern: r"x\d+".into(),
            paths: vec![root.clone()],
            ignore_case: false,
            hidden: false,
            no_ignore: false,
            threads: None,
            index: false,
            no_index: true,
            quiet: false,
            files_with_matches: false,
        };
        assert!(run(args).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
}
