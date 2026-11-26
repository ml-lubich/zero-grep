use ignore::WalkBuilder;
use memmap2::Mmap;
use rayon::prelude::*;
use regex_syntax::hir::{Hir, HirKind};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::UNIX_EPOCH,
};

const ROW_SIZE: usize = 16;

#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    len: u64,
    modified_ns: u128,
}

struct Row {
    gram: u32,
    offset: u64,
    count: u32,
}

pub struct SearchIndex {
    entries: Vec<Entry>,
    table: Mmap,
    postings: Mmap,
}

impl SearchIndex {
    pub fn open(root: &Path, rebuild: bool, hidden: bool, no_ignore: bool) -> io::Result<Self> {
        let cache = root.join(".zero-grep");
        let entries_path = cache.join("files.bin");
        let table_path = cache.join("table.bin");
        let postings_path = cache.join("postings.bin");
        if !rebuild && let Ok(index) = Self::load(&entries_path, &table_path, &postings_path) {
            return Ok(index);
        }
        Self::build(root, &cache, hidden, no_ignore)?;
        Self::load(&entries_path, &table_path, &postings_path)
    }

    fn load(entries_path: &Path, table_path: &Path, postings_path: &Path) -> io::Result<Self> {
        let entries: Vec<Entry> =
            bincode::deserialize(&fs::read(entries_path)?).map_err(io::Error::other)?;
        let table_file = File::open(table_path)?;
        let postings_file = File::open(postings_path)?;
        // Both files are immutable after atomic replacement by the builder.
        let table = unsafe { Mmap::map(&table_file)? };
        let postings = unsafe { Mmap::map(&postings_file)? };
        if table.len() % ROW_SIZE != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid n-gram table",
            ));
        }
        let (rows, _) = table.as_chunks::<ROW_SIZE>();
        for bytes in rows {
            let row = read_row(bytes);
            if row
                .offset
                .checked_add(u64::from(row.count))
                .is_none_or(|end| end > postings.len() as u64)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid posting offset",
                ));
            }
        }
        Ok(Self {
            entries,
            table,
            postings,
        })
    }

    fn build(root: &Path, cache: &Path, hidden: bool, no_ignore: bool) -> io::Result<()> {
        let mut walk = WalkBuilder::new(root);
        walk.filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".zero-grep");
        walk.hidden(!hidden)
            .ignore(!no_ignore)
            .git_ignore(!no_ignore)
            .parents(!no_ignore);
        let entries: Vec<ignore::DirEntry> = walk
            .build()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .collect();

        let files: Mutex<Vec<(Entry, Vec<u32>)>> = Mutex::new(Vec::with_capacity(entries.len()));
        entries.par_iter().for_each(|dir_entry| {
            let path = dir_entry.path();
            let Ok(meta) = dir_entry.metadata() else {
                return;
            };
            let Ok(bytes) = fs::read(path) else { return };
            let Ok(relative) = path.strip_prefix(root) else {
                return;
            };
            let modified_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            let grams: FxHashSet<u32> = if bytes.contains(&0) {
                FxHashSet::default()
            } else {
                bytes.windows(3).map(pack).collect()
            };
            files.lock().unwrap().push((
                Entry {
                    path: relative.to_string_lossy().into_owned(),
                    len: meta.len(),
                    modified_ns,
                },
                grams.into_iter().collect(),
            ));
        });

        let mut files = files.into_inner().unwrap();
        files.sort_unstable_by(|a, b| a.0.path.cmp(&b.0.path));
        let mut entries = Vec::with_capacity(files.len());
        let mut postings: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
        for (id, (entry, grams)) in files.into_iter().enumerate() {
            entries.push(entry);
            for gram in grams {
                postings.entry(gram).or_default().push(id as u32);
            }
        }

        fs::create_dir_all(cache)?;
        let suffix = format!(".{}.tmp", std::process::id());
        let entries_tmp = cache.join(format!("files.bin{suffix}"));
        let table_tmp = cache.join(format!("table.bin{suffix}"));
        let postings_tmp = cache.join(format!("postings.bin{suffix}"));
        fs::write(
            &entries_tmp,
            bincode::serialize(&entries).map_err(io::Error::other)?,
        )?;
        // Drop broad postings. A smaller subset of required grams remains a safe filter;
        // queries with only broad grams transparently fall back to scanning.
        let max_posting = (entries.len() / 5).max(1_000);
        let mut keys: Vec<u32> = postings
            .iter()
            .filter_map(|(gram, ids)| (ids.len() <= max_posting).then_some(*gram))
            .collect();
        keys.sort_unstable();
        let mut table = File::create(&table_tmp)?;
        let mut post_file = File::create(&postings_tmp)?;
        let mut offset = 0u64;
        for gram in keys {
            let ids = postings.remove(&gram).unwrap();
            let mut encoded = Vec::with_capacity(ids.len() * 2);
            let mut previous = 0u32;
            for id in &ids {
                write_varint(*id - previous, &mut encoded);
                previous = *id;
            }
            table.write_all(&gram.to_le_bytes())?;
            table.write_all(&offset.to_le_bytes())?;
            table.write_all(&(encoded.len() as u32).to_le_bytes())?;
            post_file.write_all(&encoded)?;
            offset += encoded.len() as u64;
        }
        table.sync_all()?;
        post_file.sync_all()?;
        fs::rename(entries_tmp, cache.join("files.bin"))?;
        fs::rename(table_tmp, cache.join("table.bin"))?;
        fs::rename(postings_tmp, cache.join("postings.bin"))?;
        Ok(())
    }

    pub fn candidates(&self, pattern: &str) -> Option<HashSet<String>> {
        if pattern.contains("(?i") || pattern.contains("\\n") {
            return None;
        }
        let hir = regex_syntax::Parser::new().parse(pattern).ok()?;
        let grams = required_grams(&hir);
        if grams.is_empty() {
            return Some(self.entries.iter().map(|e| e.path.clone()).collect());
        }
        let mut lists: Vec<Vec<u32>> = grams.iter().filter_map(|g| self.posting(*g)).collect();
        if lists.is_empty() {
            return None;
        }
        lists.sort_unstable_by_key(Vec::len);
        let mut ids = lists.remove(0);
        for list in &lists {
            ids.retain(|id| list.binary_search(id).is_ok());
        }
        Some(
            ids.into_iter()
                .map(|id| self.entries[id as usize].path.clone())
                .collect(),
        )
    }

    fn posting(&self, gram: u32) -> Option<Vec<u32>> {
        let rows = self.table.len() / ROW_SIZE;
        let mut low = 0usize;
        let mut high = rows;
        while low < high {
            let mid = (low + high) / 2;
            let row = read_row(&self.table[mid * ROW_SIZE..(mid + 1) * ROW_SIZE]);
            match row.gram.cmp(&gram) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => {
                    let start = row.offset as usize;
                    let end = start + row.count as usize;
                    return Some(read_varints(&self.postings[start..end]));
                }
            }
        }
        None
    }

    pub fn fresh_files(
        &self,
        root: &Path,
        hidden: bool,
        no_ignore: bool,
    ) -> (HashSet<String>, Vec<PathBuf>) {
        let known: HashMap<&str, (u64, u128)> = self
            .entries
            .iter()
            .map(|e| (e.path.as_str(), (e.len, e.modified_ns)))
            .collect();
        let state = Mutex::new((HashSet::new(), Vec::new()));
        let mut walk = WalkBuilder::new(root);
        walk.filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".zero-grep");
        walk.hidden(!hidden)
            .ignore(!no_ignore)
            .git_ignore(!no_ignore)
            .parents(!no_ignore);
        walk.build_parallel().run(|| {
            Box::new(|item| {
                let Ok(entry) = item else {
                    return ignore::WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return ignore::WalkState::Continue;
                }
                let Ok(meta) = entry.metadata() else {
                    return ignore::WalkState::Continue;
                };
                let path = entry.into_path();
                let Some(relative) = path
                    .strip_prefix(root)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
                else {
                    return ignore::WalkState::Continue;
                };
                let modified_ns = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                let mut state = state.lock().unwrap();
                if known.get(relative.as_str()) == Some(&(meta.len(), modified_ns)) {
                    state.0.insert(relative);
                } else {
                    state.1.push(path);
                }
                ignore::WalkState::Continue
            })
        });
        state.into_inner().unwrap()
    }
}

fn read_row(bytes: &[u8]) -> Row {
    Row {
        gram: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
        offset: u64::from_le_bytes(bytes[4..12].try_into().unwrap()),
        count: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
    }
}

fn required_grams(hir: &Hir) -> FxHashSet<u32> {
    match hir.kind() {
        HirKind::Literal(lit) => lit.0.windows(3).map(pack).collect(),
        HirKind::Concat(parts) => parts.iter().flat_map(required_grams).collect(),
        HirKind::Alternation(parts) => {
            let mut iter = parts.iter().map(required_grams);
            let Some(mut common) = iter.next() else {
                return FxHashSet::default();
            };
            for set in iter {
                common.retain(|g| set.contains(g));
            }
            common
        }
        HirKind::Repetition(rep) if rep.min > 0 => required_grams(&rep.sub),
        HirKind::Capture(cap) => required_grams(&cap.sub),
        _ => FxHashSet::default(),
    }
}

fn pack(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

fn write_varint(mut value: u32, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varints(bytes: &[u8]) -> Vec<u32> {
    let mut values = Vec::new();
    let (mut current, mut shift, mut previous) = (0u32, 0, 0u32);
    for byte in bytes {
        current |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            previous += current;
            values.push(previous);
            current = 0;
            shift = 0;
        } else {
            shift += 7;
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn required_grams_are_safe_across_regex_alternation() {
        let hir = regex_syntax::Parser::new()
            .parse("(?:alpha|omega)Beta")
            .unwrap();
        let grams = required_grams(&hir);
        assert!(grams.contains(&pack(b"Bet")));
        assert!(grams.contains(&pack(b"eta")));
        assert!(!grams.contains(&pack(b"pha")));
        assert!(
            required_grams(&regex_syntax::Parser::new().parse("(foobar)+").unwrap())
                .contains(&pack(b"foo"))
        );
        assert!(
            required_grams(&regex_syntax::Parser::new().parse("(?:foobar)?").unwrap()).is_empty()
        );
        assert!(required_grams(&regex_syntax::Parser::new().parse("[abc]").unwrap()).is_empty());
        assert!(required_grams(&regex_syntax::Parser::new().parse("^").unwrap()).is_empty());
    }

    #[test]
    fn delta_varints_round_trip_sorted_file_ids() {
        let ids = [0u32, 1, 127, 128, 9_000, 97_278];
        let mut bytes = Vec::new();
        let mut previous = 0;
        for id in ids {
            write_varint(id - previous, &mut bytes);
            previous = id;
        }
        assert_eq!(read_varints(&bytes), ids);
    }

    #[test]
    fn index_prunes_only_files_that_cannot_match() {
        let root = std::env::temp_dir().join(format!("zg-index-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.rs"), "alphaBeta\n").unwrap();
        fs::write(root.join("b.rs"), "omegaBeta\n").unwrap();
        fs::write(root.join("c.rs"), "other\n").unwrap();
        fs::write(root.join("binary.dat"), b"\0binary").unwrap();
        let index = SearchIndex::open(&root, true, false, false).unwrap();
        let candidates = index.candidates("(?:alpha|omega)Beta").unwrap();
        assert!(candidates.contains("a.rs"));
        assert!(candidates.contains("b.rs"));
        assert!(!candidates.contains("c.rs"));
        assert!(index.candidates("(").is_none());
        assert!(index.candidates("not-present-at-all").is_none());
        assert!(SearchIndex::open(&root, false, false, false).is_ok());
        let (fresh, dirty) = index.fresh_files(&root, false, false);
        assert_eq!(fresh.len(), 4);
        assert!(dirty.is_empty());
        assert_eq!(index.candidates("(?i)Beta"), None);
        assert_eq!(index.candidates("xy").unwrap().len(), 4);
        fs::write(root.join("a.rs"), "changedBeta\n").unwrap();
        fs::write(root.join("new.rs"), "freshBeta\n").unwrap();
        let (fresh, dirty) = index.fresh_files(&root, false, false);
        assert_eq!(fresh.len(), 3);
        assert_eq!(dirty.len(), 2);
        drop(index);

        let table = root.join(".zero-grep/table.bin");
        fs::write(&table, [0]).unwrap();
        assert!(SearchIndex::open(&root, false, false, false).is_ok());
        let mut bytes = fs::read(&table).unwrap();
        bytes[4..12].copy_from_slice(&u64::MAX.to_le_bytes());
        fs::write(&table, bytes).unwrap();
        let rebuilt = SearchIndex::open(&root, false, false, false).unwrap();
        assert!(rebuilt.candidates("changedBeta").unwrap().contains("a.rs"));
        drop(rebuilt);
        fs::remove_dir_all(root).unwrap();
    }
}
