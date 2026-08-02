use crate::content_manifest::ContentManifest;
use crate::depot_config::{atomic_write_synced, DepotConfigStore, INVALID_MANIFEST_ID};
use crate::depot_downloader::ResolvedDepotSpec;
use crate::depot_writer::DEPOT_FILE_FLAG_DIRECTORY;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const STALE_CLEANUP_SUFFIX: &str = ".stalecleanup";
const FILELIST_SUFFIX: &str = ".filelist";
const FILELIST_HEADER: &str = "WNFL1";

fn cleanup_log(message: &str) {
    crate::jni::android_log("WnSteamDepotCleanup", message);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
}

pub fn stale_cleanup_marker_name(depot_id: u32, manifest_id: u64) -> String {
    format!("{depot_id}_{manifest_id}{STALE_CLEANUP_SUFFIX}")
}

pub fn stale_cleanup_marker_path(
    config_dir: impl AsRef<Path>,
    depot_id: u32,
    manifest_id: u64,
) -> PathBuf {
    config_dir
        .as_ref()
        .join(stale_cleanup_marker_name(depot_id, manifest_id))
}

pub fn filelist_sidecar_path(
    config_dir: impl AsRef<Path>,
    depot_id: u32,
    manifest_id: u64,
) -> PathBuf {
    config_dir
        .as_ref()
        .join(format!("{depot_id}_{manifest_id}{FILELIST_SUFFIX}"))
}

/// Persists a manifest's decrypted file list next to its cache, so the pass can
/// read a depot it holds no key for. Written only for depots that reach this
/// downloader, so an install predating sidecars has none until each depot has
/// been downloaded once (a full Verify Files does them all); until then the
/// keep-union is incomplete and the pass correctly defers.
pub fn write_filelist_sidecar(
    config_dir: impl AsRef<Path>,
    depot_id: u32,
    manifest_id: u64,
    manifest: &ContentManifest,
) -> bool {
    let mut body = String::with_capacity(manifest.files.len() * 32);
    let mut written = 0usize;
    for file in &manifest.files {
        if file.filename.contains('\n') || file.filename.contains('\r') {
            continue;
        }
        written += 1;
        let kind = if (file.flags & DEPOT_FILE_FLAG_DIRECTORY) != 0 {
            'D'
        } else {
            'F'
        };
        body.push(kind);
        body.push(' ');
        body.push_str(&file.filename);
        body.push('\n');
    }
    // Entry count in the header: a truncated sidecar for an installed depot would
    // otherwise under-populate the keep-union and the difference gets deleted.
    let blob = format!("{FILELIST_HEADER} {written}\n{body}");
    atomic_write_synced(
        &filelist_sidecar_path(config_dir, depot_id, manifest_id),
        blob.as_bytes(),
    )
}

/// Writes the sidecar for an already-installed depot that predates sidecars,
/// using the cached manifest and this operation's key. No-op when the sidecar
/// exists or the manifest cache is unreadable.
pub fn backfill_filelist_sidecar(config_dir: &Path, depot: &ResolvedDepotSpec) {
    if filelist_sidecar_path(config_dir, depot.depot_id, depot.manifest_id).is_file() {
        return;
    }
    if let Some(manifest) = load_manifest(
        config_dir,
        depot.depot_id,
        depot.manifest_id,
        &depot.depot_key,
    ) {
        let _ = write_filelist_sidecar(config_dir, depot.depot_id, depot.manifest_id, &manifest);
    }
}

fn read_filelist_sidecar(
    config_dir: &Path,
    depot_id: u32,
    manifest_id: u64,
) -> Option<Vec<FileEntry>> {
    let blob = fs::read_to_string(filelist_sidecar_path(config_dir, depot_id, manifest_id)).ok()?;
    let mut lines = blob.lines();
    let expected: usize = lines.next()?.strip_prefix(FILELIST_HEADER)?.trim().parse().ok()?;
    let mut entries = Vec::new();
    for line in lines {
        let (kind, name) = match (line.get(..2), line.get(2..)) {
            // An empty name is a torn last line, not an entry — dropping it lets
            // the count check below catch the truncation.
            (Some("F "), Some(name)) if !name.is_empty() => (false, name),
            (Some("D "), Some(name)) if !name.is_empty() => (true, name),
            _ => continue,
        };
        entries.push(FileEntry {
            name: name.to_string(),
            is_dir: kind,
        });
    }
    if entries.len() != expected {
        cleanup_log(&format!(
            "cleanup: file list {depot_id}_{manifest_id} is truncated \
             ({} of {expected} entries), treating as unreadable",
            entries.len()
        ));
        return None;
    }
    Some(entries)
}

/// Records that a depot is about to move off `old_manifest_id`, so the files
/// the old manifest installed can be diffed away once the new build is fully
/// on disk. The marker survives pause/cancel — cleanup only ever runs after a
/// fully successful download, and a leftover marker is retried then.
pub fn record_pending_cleanup(
    config_dir: impl AsRef<Path>,
    depot_id: u32,
    old_manifest_id: u64,
    new_manifest_id: u64,
) -> bool {
    if old_manifest_id == 0
        || old_manifest_id == INVALID_MANIFEST_ID
        || old_manifest_id == new_manifest_id
    {
        return false;
    }
    write_cleanup_marker(config_dir, depot_id, old_manifest_id)
}

/// Marks a build whose write started but never committed — its files can sit on
/// disk with no depot.config entry to ever diff them away. Dropped if the build
/// later commits (resume), else reclaimed by the next successful download.
pub fn record_aborted_build(config_dir: impl AsRef<Path>, depot_id: u32, manifest_id: u64) -> bool {
    if manifest_id == 0 || manifest_id == INVALID_MANIFEST_ID {
        return false;
    }
    write_cleanup_marker(config_dir, depot_id, manifest_id)
}

fn write_cleanup_marker(config_dir: impl AsRef<Path>, depot_id: u32, manifest_id: u64) -> bool {
    let path = stale_cleanup_marker_path(config_dir, depot_id, manifest_id);
    let Some(parent) = path.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    fs::write(path, manifest_id.to_string()).is_ok()
}

pub fn pending_cleanup_markers(config_dir: impl AsRef<Path>) -> Vec<(u32, u64)> {
    let Ok(entries) = fs::read_dir(config_dir.as_ref()) else {
        return Vec::new();
    };
    let mut markers = BTreeSet::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(stem) = name.strip_suffix(STALE_CLEANUP_SUFFIX) else {
            continue;
        };
        let mut parts = stem.split('_');
        let (Some(depot), Some(gid), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if let (Ok(depot_id), Ok(manifest_id)) = (depot.parse::<u32>(), gid.parse::<u64>()) {
            markers.insert((depot_id, manifest_id));
        }
    }
    markers.into_iter().collect()
}

fn remove_cleanup_marker(config_dir: impl AsRef<Path>, depot_id: u32, manifest_id: u64) {
    let _ = fs::remove_file(stale_cleanup_marker_path(config_dir, depot_id, manifest_id));
}

/// Deletes files an old manifest installed that no current manifest references —
/// the gap left because a download only ever writes new content.
///
/// Candidates come only from cached manifest file lists, minus the union of
/// every installed manifest's files, so anything absent from all Steam manifests
/// can never be a candidate — saves, steam_settings/ and user-added mod files
/// are structurally safe. The exception is a Steamless `<name>.exe.original.exe`
/// backup, deleted with its primary so a stale patch can't be restored.
///
/// Marker-driven builds are the guarantee; unmarked cached builds are swept
/// best-effort, since pruneStaleDepotManifestCache on the Kotlin side reclaims
/// those same cached lists once depot.config no longer references them.
/// Returns the number of files deleted.
pub fn run_stale_file_cleanup(
    install_dir: &str,
    config_dir: &Path,
    depots: &[ResolvedDepotSpec],
) -> u32 {
    let keys: BTreeMap<u32, &[u8]> = depots
        .iter()
        .map(|depot| (depot.depot_id, depot.depot_key.as_slice()))
        .collect();
    let cfg = DepotConfigStore::load(config_dir);

    // Every old build still on disk: each cached filelist/manifest plus any
    // pending marker. Sweeping cached builds (not just marked ones) reclaims
    // orphans even when a marker was dropped or never recorded.
    let mut old_builds = collect_cached_builds(config_dir);
    old_builds.extend(pending_cleanup_markers(config_dir));
    if old_builds.is_empty() {
        return 0;
    }

    // The keep-union must be COMPLETE: proving a candidate is dead requires
    // reading every installed depot, so if one is in progress or unreadable,
    // defer the whole pass rather than risk deleting a file that moved depots.
    let mut keep = BTreeSet::new();
    let mut current: BTreeMap<u32, u64> = BTreeMap::new();
    for (depot_id, gid) in cfg.installed_entries() {
        if gid == INVALID_MANIFEST_ID {
            cleanup_log(&format!(
                "cleanup: depot {depot_id} in progress, deferring stale-file pass"
            ));
            return 0;
        }
        match load_file_entries(config_dir, depot_id, gid, keys.get(&depot_id).copied()) {
            Some(entries) => {
                for entry in &entries {
                    keep.insert(normalized_key(&entry.name));
                }
                current.insert(depot_id, gid);
            }
            None => {
                cleanup_log(&format!(
                    "cleanup: installed depot {depot_id}_{gid} file list unreadable, \
                     deferring stale-file pass"
                ));
                return 0;
            }
        }
    }

    // An unreadable or absent depot.config loads as an EMPTY store, which is
    // indistinguishable from "nothing installed" — and an empty keep-union would
    // condemn every cached build, including the one just downloaded.
    if current.is_empty() {
        cleanup_log("cleanup: no installed depots readable, deferring stale-file pass");
        return 0;
    }

    let install_root = Path::new(install_dir);
    let mut deleted = 0u32;
    for (depot_id, gid) in old_builds {
        if current.get(&depot_id) == Some(&gid) {
            // This build is the one currently installed — not stale.
            remove_cleanup_marker(config_dir, depot_id, gid);
            continue;
        }
        // Absent from depot.config means UNKNOWN, not dead: a fresh run forgets
        // its depots before downloading them, so files can be on disk with no
        // entry. Only a depot we can see the current build of can be diffed.
        if !current.contains_key(&depot_id) {
            cleanup_log(&format!(
                "cleanup: depot {depot_id} not in depot.config, deferring its builds"
            ));
            continue;
        }
        let Some(entries) =
            load_file_entries(config_dir, depot_id, gid, keys.get(&depot_id).copied())
        else {
            if filelist_sidecar_path(config_dir, depot_id, gid).is_file()
                || config_dir
                    .join(format!("{depot_id}_{gid}.manifest"))
                    .is_file()
            {
                // The data is on disk but this op can't read it (no key yet);
                // a later op that carries the key finishes the job.
                cleanup_log(&format!(
                    "cleanup: old file list {depot_id}_{gid} unreadable in this op, deferring"
                ));
            } else {
                cleanup_log(&format!(
                    "cleanup: old file list {depot_id}_{gid} is gone, dropping marker"
                ));
                remove_cleanup_marker(config_dir, depot_id, gid);
            }
            continue;
        };
        deleted += delete_build_orphans(install_root, depot_id, gid, &entries, &keep);
        remove_cleanup_marker(config_dir, depot_id, gid);
    }
    if deleted > 0 {
        cleanup_log(&format!(
            "cleanup: removed {deleted} stale file(s) under '{install_dir}'"
        ));
    }
    deleted
}

/// Deletes the files of one old build that no current build still ships.
fn delete_build_orphans(
    install_root: &Path,
    depot_id: u32,
    gid: u64,
    entries: &[FileEntry],
    keep: &BTreeSet<String>,
) -> u32 {
    let mut deleted = 0u32;
    let mut dirs = BTreeSet::new();
    for entry in entries {
        let key = normalized_key(&entry.name);
        if keep.contains(&key) {
            continue;
        }
        let Some(parts) = sanitized_components(&entry.name) else {
            cleanup_log(&format!(
                "cleanup: refusing unsafe manifest path '{}'",
                entry.name
            ));
            continue;
        };
        if has_symlinked_ancestor(install_root, &parts) {
            cleanup_log(&format!(
                "cleanup: '{}' is behind a symlinked directory, skipping",
                entry.name
            ));
            continue;
        }
        let mut path = install_root.to_path_buf();
        path.extend(&parts);
        if entry.is_dir {
            dirs.insert(path);
            continue;
        }
        if delete_stale_file(&path) {
            cleanup_log(&format!(
                "cleanup: deleted '{}' (depot {depot_id}, gone after manifest {gid})",
                path.display()
            ));
            deleted += 1;
            // Steamless backs up patched exes as "<name>.original.exe";
            // restoreOriginalExecutable would resurrect a deleted exe from an
            // orphaned backup, so the backup goes with its primary — but only
            // an exe's backup, and never one a current manifest legitimately
            // ships.
            if key.ends_with(".exe") && !keep.contains(&format!("{key}.original.exe")) {
                let backup = sibling_original_backup(&path);
                if delete_stale_file(&backup) {
                    cleanup_log(&format!("cleanup: deleted backup '{}'", backup.display()));
                    deleted += 1;
                }
            }
            if let Some(parent) = path.parent() {
                if parent != install_root {
                    dirs.insert(parent.to_path_buf());
                }
            }
        }
    }
    for dir in dirs.iter().rev() {
        prune_empty_dirs_up(install_root, dir, keep);
    }
    deleted
}

/// Every (depot_id, gid) with a cached filelist sidecar or manifest — the set
/// of builds whose file lists this pass can diff against the current install.
fn collect_cached_builds(config_dir: &Path) -> BTreeSet<(u32, u64)> {
    let mut builds = BTreeSet::new();
    let Ok(entries) = fs::read_dir(config_dir) else {
        return builds;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let stem = match name.strip_suffix(FILELIST_SUFFIX) {
            Some(stem) => stem,
            None => match name.strip_suffix(".manifest") {
                Some(stem) => stem,
                None => continue,
            },
        };
        let mut parts = stem.split('_');
        if let (Some(depot), Some(gid), None) = (parts.next(), parts.next(), parts.next()) {
            if let (Ok(depot_id), Ok(manifest_id)) = (depot.parse::<u32>(), gid.parse::<u64>()) {
                builds.insert((depot_id, manifest_id));
            }
        }
    }
    builds
}

fn load_manifest(
    config_dir: &Path,
    depot_id: u32,
    manifest_id: u64,
    depot_key: &[u8],
) -> Option<ContentManifest> {
    let path = config_dir.join(format!("{depot_id}_{manifest_id}.manifest"));
    let raw = fs::read(path).ok()?;
    if raw.is_empty() {
        return None;
    }
    let mut manifest = ContentManifest::parse(&raw)?;
    manifest.decrypt_filenames(depot_key).then_some(manifest)
}

/// Sidecar first (key-independent), then the cached manifest when this
/// operation holds the depot key.
fn load_file_entries(
    config_dir: &Path,
    depot_id: u32,
    manifest_id: u64,
    depot_key: Option<&[u8]>,
) -> Option<Vec<FileEntry>> {
    if let Some(entries) = read_filelist_sidecar(config_dir, depot_id, manifest_id) {
        return Some(entries);
    }
    let manifest = load_manifest(config_dir, depot_id, manifest_id, depot_key?)?;
    Some(
        manifest
            .files
            .iter()
            .map(|file| FileEntry {
                name: file.filename.clone(),
                is_dir: (file.flags & DEPOT_FILE_FLAG_DIRECTORY) != 0,
            })
            .collect(),
    )
}

/// Canonical comparison key: separators are already '/' after
/// decrypt_filenames; "."/empty components are dropped (the writer accepts
/// "./a" and "a//b" spellings) and case is folded so both sides of the
/// old-minus-current diff normalize identically.
fn normalized_key(rel: &str) -> String {
    let mut key = String::with_capacity(rel.len());
    for part in rel.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if !key.is_empty() {
            key.push('/');
        }
        key.push_str(&part.to_ascii_lowercase());
    }
    key
}

/// Path components safe to delete under the install dir: plain relative
/// paths only; "."/empty components are dropped to mirror normalized_key.
/// Stricter than depot_writer::path_is_safe, which permits ':' — a file it
/// would write is then never reclaimed here. Unreachable with real Steam
/// manifests, and erring toward not deleting is the right way to be wrong.
fn sanitized_components(rel: &str) -> Option<Vec<&str>> {
    if rel.starts_with('/') || rel.contains(':') {
        return None;
    }
    let mut parts = Vec::new();
    for part in rel.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.bytes().any(|b| b.is_ascii_control()) {
            return None;
        }
        parts.push(part);
    }
    if parts.is_empty() || parts[0].eq_ignore_ascii_case(".DepotDownloader") {
        return None;
    }
    Some(parts)
}

fn delete_stale_file(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.is_dir() {
        // Manifest called it a file but the disk has a directory — leave it.
        return false;
    }
    fs::remove_file(path).is_ok()
}

fn sibling_original_backup(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".original.exe");
    PathBuf::from(name)
}

/// Manifest-created symlinks may target arbitrary paths; deleting through one
/// would escape the install dir, so candidates behind a symlinked directory
/// are left alone.
fn has_symlinked_ancestor(install_root: &Path, parts: &[&str]) -> bool {
    let mut current = install_root.to_path_buf();
    for part in &parts[..parts.len().saturating_sub(1)] {
        current.push(part);
        let is_symlink = fs::symlink_metadata(&current)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            return true;
        }
    }
    false
}

/// [keep] guards directories a current manifest ships empty (Mods/, Saves/):
/// games that assume they exist break if the old build's files leave them bare.
fn prune_empty_dirs_up(install_root: &Path, start: &Path, keep: &BTreeSet<String>) {
    let mut current = start;
    while current != install_root && current.starts_with(install_root) {
        let rel = current.strip_prefix(install_root).ok().and_then(|p| p.to_str());
        if rel.is_some_and(|r| keep.contains(&normalized_key(r))) {
            break;
        }
        if fs::remove_dir(current).is_err() {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_manifest::{END_OF_MANIFEST_MAGIC, METADATA_MAGIC, PAYLOAD_MAGIC};
    use crate::depot_writer::DEPOT_FILE_FLAG_EXECUTABLE;
    use crate::proto_wire::Writer;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wnsteam_cleanup_{name}_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn raw_manifest(depot_id: u32, manifest_id: u64, files: &[(&str, u32)]) -> Vec<u8> {
        let mut payload = Vec::new();
        for (filename, flags) in files {
            let mut file_body = Vec::new();
            {
                let mut writer = Writer::new(&mut file_body);
                writer.string_field(1, filename);
                writer.uint64_field(2, 1);
                writer.uint32_field(3, *flags);
            }
            Writer::new(&mut payload).submessage_field(1, &file_body);
        }

        let mut metadata = Vec::new();
        {
            let mut writer = Writer::new(&mut metadata);
            writer.uint32_field(1, depot_id);
            writer.uint64_field(2, manifest_id);
            writer.bool_field_force(4, false);
        }

        let mut raw = Vec::new();
        push_section(&mut raw, PAYLOAD_MAGIC, &payload);
        push_section(&mut raw, METADATA_MAGIC, &metadata);
        raw.extend_from_slice(&END_OF_MANIFEST_MAGIC.to_le_bytes());
        raw
    }

    fn push_section(out: &mut Vec<u8>, magic: u32, body: &[u8]) {
        out.extend_from_slice(&magic.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
    }

    fn write_manifest(config_dir: &Path, depot_id: u32, manifest_id: u64, files: &[(&str, u32)]) {
        fs::write(
            config_dir.join(format!("{depot_id}_{manifest_id}.manifest")),
            raw_manifest(depot_id, manifest_id, files),
        )
        .unwrap();
    }

    fn write_sidecar(config_dir: &Path, depot_id: u32, manifest_id: u64, files: &[(&str, u32)]) {
        let manifest = ContentManifest::parse(&raw_manifest(depot_id, manifest_id, files)).unwrap();
        assert!(write_filelist_sidecar(
            config_dir,
            depot_id,
            manifest_id,
            &manifest
        ));
    }

    fn touch(install: &Path, rel: &str) {
        let path = install.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    fn spec(depot_id: u32, manifest_id: u64) -> ResolvedDepotSpec {
        ResolvedDepotSpec {
            depot_id,
            manifest_id,
            depot_key: vec![1u8; 32],
            manifest_request_code: 0,
        }
    }

    fn config_dir(install: &Path) -> PathBuf {
        let dir = install.join(".DepotDownloader");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn install_current(config_dir: &Path, depot_id: u32, manifest_id: u64) {
        let mut cfg = DepotConfigStore::load(config_dir);
        cfg.finish_depot(depot_id, manifest_id);
    }

    #[test]
    fn marker_recording_skips_noop_transitions() {
        let dir = temp_dir("marker_noop");
        assert!(!record_pending_cleanup(&dir, 100, 0, 555));
        assert!(!record_pending_cleanup(&dir, 100, INVALID_MANIFEST_ID, 555));
        assert!(!record_pending_cleanup(&dir, 100, 555, 555));
        assert!(pending_cleanup_markers(&dir).is_empty());

        assert!(record_pending_cleanup(&dir, 100, 444, 555));
        assert_eq!(pending_cleanup_markers(&dir), vec![(100, 444)]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn filelist_sidecar_roundtrips_files_and_dirs() {
        let dir = temp_dir("sidecar_roundtrip");
        write_sidecar(
            &dir,
            100,
            555,
            &[("bin", DEPOT_FILE_FLAG_DIRECTORY), ("bin/game.exe", 0)],
        );
        let entries = read_filelist_sidecar(&dir, 100, 555).unwrap();
        assert_eq!(
            entries,
            vec![
                FileEntry {
                    name: "bin".into(),
                    is_dir: true
                },
                FileEntry {
                    name: "bin/game.exe".into(),
                    is_dir: false
                },
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn deletes_files_dropped_by_new_manifest_and_prunes_dirs() {
        let install = temp_dir("manifest_switch");
        let config = config_dir(&install);
        write_manifest(
            &config,
            100,
            444,
            &[
                ("bin", DEPOT_FILE_FLAG_DIRECTORY),
                ("bin/old", DEPOT_FILE_FLAG_DIRECTORY),
                ("bin/old/legacy.dll", 0),
                ("bin/game.exe", DEPOT_FILE_FLAG_EXECUTABLE),
                ("data.pak", 0),
            ],
        );
        write_manifest(
            &config,
            100,
            555,
            &[
                ("bin", DEPOT_FILE_FLAG_DIRECTORY),
                ("bin/game.exe", DEPOT_FILE_FLAG_EXECUTABLE),
                ("data.pak", 0),
            ],
        );
        install_current(&config, 100, 555);
        touch(&install, "bin/old/legacy.dll");
        touch(&install, "bin/game.exe");
        touch(&install, "data.pak");
        touch(&install, "steam_settings/configs.app.ini");
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 1);
        assert!(!install.join("bin/old/legacy.dll").exists());
        assert!(!install.join("bin/old").exists());
        assert!(install.join("bin/game.exe").exists());
        assert!(install.join("data.pak").exists());
        assert!(install.join("steam_settings/configs.app.ini").exists());
        assert!(install.exists());
        assert!(pending_cleanup_markers(&config).is_empty());
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn narrowed_update_cleans_via_sidecars_without_other_depots_keys() {
        // A narrowed update op carries only the changed depot; the other
        // installed depot is represented by its sidecar alone.
        let install = temp_dir("narrowed_update");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 444, &[("shared.dat", 0), ("only_old.dat", 0)]);
        write_sidecar(&config, 100, 555, &[("core.dat", 0)]);
        write_sidecar(&config, 200, 777, &[("Shared.dat", 0)]);
        install_current(&config, 100, 555);
        install_current(&config, 200, 777);
        touch(&install, "shared.dat");
        touch(&install, "only_old.dat");
        touch(&install, "core.dat");
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 1);
        assert!(
            install.join("shared.dat").exists(),
            "depot 200 still ships it"
        );
        assert!(!install.join("only_old.dat").exists());
        assert!(install.join("core.dat").exists());
        assert!(pending_cleanup_markers(&config).is_empty());
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn unreadable_old_list_with_data_on_disk_defers_marker() {
        let install = temp_dir("defer_unreadable_old");
        let config = config_dir(&install);
        write_manifest(&config, 100, 444, &[("only_old.dat", 0)]);
        write_sidecar(&config, 100, 555, &[("core.dat", 0)]);
        install_current(&config, 100, 555);
        touch(&install, "only_old.dat");
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        // Old manifest exists but this op has no key for depot 100 (and no
        // old sidecar) → defer, keep the marker for an op that has the key.
        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(200, 777)]);
        assert_eq!(deleted, 0);
        assert!(install.join("only_old.dat").exists());
        assert_eq!(pending_cleanup_markers(&config), vec![(100, 444)]);

        // Once the data is gone entirely the marker can never act → dropped.
        fs::remove_file(config.join("100_444.manifest")).unwrap();
        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(200, 777)]);
        assert_eq!(deleted, 0);
        assert!(pending_cleanup_markers(&config).is_empty());
        assert!(install.join("only_old.dat").exists());
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn unreadable_installed_depot_defers_whole_pass() {
        // An installed depot whose file list can't be read (no sidecar, no key
        // in this op) leaves the keep-union incomplete, so the pass defers
        // rather than risk deleting a live cross-depot file.
        let install = temp_dir("unreadable_defers");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 444, &[("only_old.dat", 0)]);
        write_sidecar(&config, 100, 555, &[("core.dat", 0)]);
        install_current(&config, 100, 555);
        install_current(&config, 200, 777); // no sidecar, no key in op
        touch(&install, "only_old.dat");
        touch(&install, "core.dat");
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 0);
        assert!(install.join("only_old.dat").exists());
        assert!(install.join("core.dat").exists());
        assert_eq!(pending_cleanup_markers(&config), vec![(100, 444)]);
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn unreadable_depot_config_defers_instead_of_deleting_everything() {
        // DepotConfigStore::load yields an EMPTY store on a corrupt/unreadable
        // depot.config, which must not read as "nothing is installed" — an empty
        // keep-union would condemn every cached build, the fresh one included.
        let install = temp_dir("unreadable_config_defer");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 555, &[("core.dat", 0)]);
        write_sidecar(&config, 100, 444, &[("only_old.dat", 0)]);
        touch(&install, "core.dat");
        touch(&install, "only_old.dat");
        assert!(record_pending_cleanup(&config, 100, 444, 555));
        // Corrupt depot.config: parses to an empty store.
        fs::write(config.join("depot.config"), b"{ not json").unwrap();

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 0, "must defer, not sweep on an unreadable config");
        assert!(install.join("core.dat").exists(), "fresh build must survive");
        assert!(install.join("only_old.dat").exists());
        assert_eq!(pending_cleanup_markers(&config), vec![(100, 444)]);
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn depot_absent_from_config_defers_its_builds() {
        // A fresh run forgets its depots up front, so files can sit on disk with
        // no depot.config entry. Absent means UNKNOWN, not dead.
        let install = temp_dir("absent_depot_defer");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 555, &[("core.dat", 0)]);
        // Depot 200 has files + a sidecar but was forgotten from depot.config.
        write_sidecar(&config, 200, 777, &[("dlc.dat", 0)]);
        install_current(&config, 100, 555);
        touch(&install, "core.dat");
        touch(&install, "dlc.dat");

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 0, "forgotten depot's files are unknown, not stale");
        assert!(install.join("dlc.dat").exists(), "must not wipe a forgotten depot");
        assert!(install.join("core.dat").exists());
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn keeps_empty_directory_the_current_manifest_ships() {
        // A manifest that ships an empty Mods/ must keep it even when the old
        // build's files were the only things in it.
        let install = temp_dir("keep_shipped_empty_dir");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 444, &[("Mods/old_mod.dat", 0)]);
        write_sidecar(
            &config,
            100,
            555,
            &[("core.dat", 0), ("Mods", DEPOT_FILE_FLAG_DIRECTORY)],
        );
        install_current(&config, 100, 555);
        touch(&install, "core.dat");
        touch(&install, "Mods/old_mod.dat");
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 1);
        assert!(!install.join("Mods/old_mod.dat").exists());
        assert!(install.join("Mods").is_dir(), "shipped empty dir must survive");
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn truncated_file_list_defers_instead_of_under_populating_keep() {
        let install = temp_dir("truncated_sidecar_defer");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 444, &[("only_old.dat", 0)]);
        write_sidecar(&config, 100, 555, &[("core.dat", 0), ("extra.dat", 0)]);
        install_current(&config, 100, 555);
        touch(&install, "only_old.dat");
        touch(&install, "core.dat");
        touch(&install, "extra.dat");
        assert!(record_pending_cleanup(&config, 100, 444, 555));
        // Lop the last entry off the INSTALLED build's sidecar.
        let path = filelist_sidecar_path(&config, 100, 555);
        let full = fs::read_to_string(&path).unwrap();
        fs::write(&path, full.rsplit_once("extra.dat").unwrap().0).unwrap();

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 0, "an incomplete keep-union must defer the pass");
        assert!(install.join("only_old.dat").exists());
        assert!(install.join("extra.dat").exists());
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn in_progress_depot_defers_whole_pass() {
        // An in-flight (INVALID) depot's target build is unknown to cleanup, so
        // a file moving into it can't be proven safe — the whole pass defers
        // until that depot finishes.
        let install = temp_dir("in_progress_defer");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 444, &[("only_old.dat", 0)]);
        write_sidecar(&config, 100, 555, &[("core.dat", 0)]);
        write_sidecar(&config, 200, 777, &[("dlc.dat", 0)]);
        install_current(&config, 100, 555);
        let mut cfg = DepotConfigStore::load(&config);
        cfg.begin_depot(200); // 200 -> INVALID (mid-download)
        touch(&install, "only_old.dat");
        touch(&install, "core.dat");
        touch(&install, "dlc.dat");
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 0, "deferred while depot 200 is in flight");
        assert!(install.join("only_old.dat").exists());
        assert!(install.join("dlc.dat").exists());
        assert_eq!(pending_cleanup_markers(&config), vec![(100, 444)]);
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn cross_depot_file_moving_into_in_flight_depot_survives() {
        // Regression: shared.dat was in depot 100's old build 444 and is not in
        // its current build 555, but the in-flight depot 200 ships it. Cleanup
        // can't see 200's target build, so it must not delete shared.dat.
        let install = temp_dir("cross_depot_in_flight");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 444, &[("shared.dat", 0), ("only_old.dat", 0)]);
        write_sidecar(&config, 100, 555, &[("core.dat", 0)]);
        install_current(&config, 100, 555);
        let mut cfg = DepotConfigStore::load(&config);
        cfg.begin_depot(200); // 200 mid-download (will ship shared.dat), unreadable
        touch(&install, "shared.dat");
        touch(&install, "core.dat");
        touch(&install, "only_old.dat");
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 0);
        assert!(
            install.join("shared.dat").exists(),
            "live file shipped by the in-flight depot must survive"
        );
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn sweeps_cached_old_build_without_a_marker() {
        // The defining B behavior: an aborted build's files are reclaimed from
        // its cached sidecar even when no .stalecleanup marker exists.
        let install = temp_dir("sweep_no_marker");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 555, &[("core.dat", 0)]);
        write_sidecar(&config, 100, 777, &[("core.dat", 0), ("dropped_in_patch.dll", 0)]);
        install_current(&config, 100, 555);
        touch(&install, "core.dat");
        touch(&install, "dropped_in_patch.dll"); // left by an aborted update to 777
        touch(&install, "user_mod.cfg"); // user-added, in no manifest
                                         // No marker recorded for 777.
        assert!(pending_cleanup_markers(&config).is_empty());

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 1);
        assert!(
            !install.join("dropped_in_patch.dll").exists(),
            "aborted build's file swept"
        );
        assert!(install.join("core.dat").exists());
        assert!(
            install.join("user_mod.cfg").exists(),
            "user-added mod preserved"
        );
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn normalization_matches_dotted_and_doubled_separator_spellings() {
        assert_eq!(normalized_key("./Bin//Game.EXE"), "bin/game.exe");
        assert_eq!(normalized_key("bin/game.exe"), "bin/game.exe");

        // Old spelled plainly, new spelled with "./" — still the same file.
        let install = temp_dir("normalized_keep");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 444, &[("bin/x.dll", 0), ("only_old.dat", 0)]);
        write_sidecar(&config, 100, 555, &[("./bin//x.dll", 0)]);
        install_current(&config, 100, 555);
        touch(&install, "bin/x.dll");
        touch(&install, "only_old.dat");
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);
        assert_eq!(deleted, 1);
        assert!(install.join("bin/x.dll").exists());
        assert!(!install.join("only_old.dat").exists());
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn rejects_unsafe_manifest_paths() {
        assert_eq!(
            sanitized_components("bin/game.exe"),
            Some(vec!["bin", "game.exe"])
        );
        assert_eq!(
            sanitized_components("./bin//game.exe"),
            Some(vec!["bin", "game.exe"])
        );
        assert_eq!(sanitized_components(""), None);
        assert_eq!(sanitized_components("."), None);
        assert_eq!(sanitized_components("/etc/passwd"), None);
        assert_eq!(sanitized_components("../outside.dat"), None);
        assert_eq!(sanitized_components("bin/../../outside.dat"), None);
        assert_eq!(sanitized_components("c:/windows/system32"), None);
        assert_eq!(sanitized_components(".DepotDownloader/depot.config"), None);
        assert_eq!(sanitized_components(".depotdownloader/depot.config"), None);
        assert_eq!(sanitized_components("bad\nname"), None);
    }

    #[test]
    fn deletes_steamless_backup_only_for_unkept_exe_primaries() {
        let install = temp_dir("steamless_backup");
        let config = config_dir(&install);
        write_manifest(
            &config,
            100,
            444,
            &[("old.exe", 0), ("game.exe", 0), ("data.pak", 0)],
        );
        write_manifest(&config, 100, 555, &[("game.exe", 0)]);
        install_current(&config, 100, 555);
        touch(&install, "old.exe");
        touch(&install, "old.exe.original.exe");
        touch(&install, "game.exe");
        touch(&install, "game.exe.original.exe");
        touch(&install, "data.pak");
        touch(&install, "data.pak.original.exe"); // not an exe primary
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 3); // old.exe + its backup + data.pak
        assert!(!install.join("old.exe").exists());
        assert!(!install.join("old.exe.original.exe").exists());
        assert!(install.join("game.exe").exists());
        assert!(install.join("game.exe.original.exe").exists());
        assert!(!install.join("data.pak").exists());
        assert!(
            install.join("data.pak.original.exe").exists(),
            "backup deletion is exe-only"
        );
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn keeps_backup_shipped_by_current_manifest() {
        let install = temp_dir("kept_backup");
        let config = config_dir(&install);
        write_manifest(&config, 100, 444, &[("tool.exe", 0)]);
        write_manifest(
            &config,
            100,
            555,
            &[("tool.exe.original.exe", 0), ("core.dat", 0)],
        );
        install_current(&config, 100, 555);
        touch(&install, "tool.exe");
        touch(&install, "tool.exe.original.exe");
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 1);
        assert!(!install.join("tool.exe").exists());
        assert!(
            install.join("tool.exe.original.exe").exists(),
            "current manifest ships this exact name"
        );
        let _ = fs::remove_dir_all(&install);
    }

    #[cfg(unix)]
    #[test]
    fn skips_candidates_behind_symlinked_directories() {
        let install = temp_dir("symlink_ancestor");
        let config = config_dir(&install);
        let outside = temp_dir("symlink_target");
        fs::write(outside.join("precious.dat"), b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, install.join("link")).unwrap();

        write_manifest(&config, 100, 444, &[("link/precious.dat", 0)]);
        write_manifest(&config, 100, 555, &[("core.dat", 0)]);
        install_current(&config, 100, 555);
        assert!(record_pending_cleanup(&config, 100, 444, 555));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 0);
        assert!(outside.join("precious.dat").exists());
        assert!(install.join("link").exists());
        let _ = fs::remove_dir_all(&install);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn aborted_build_marker_reclaims_partial_files_after_revert() {
        let install = temp_dir("aborted_revert");
        let config = config_dir(&install);
        // Build 555 committed; the update to 777 was cancelled mid-write
        // after the sidecar was written and one B-only file landed on disk.
        write_sidecar(&config, 100, 555, &[("game.exe", 0), ("data.pak", 0)]);
        write_sidecar(&config, 100, 777, &[("game.exe", 0), ("dropped_in_patch.dll", 0)]);
        install_current(&config, 100, 555);
        touch(&install, "game.exe");
        touch(&install, "data.pak");
        touch(&install, "dropped_in_patch.dll");
        assert!(record_aborted_build(&config, 100, 777));
        assert!(!record_aborted_build(&config, 100, 0));
        assert!(!record_aborted_build(&config, 100, INVALID_MANIFEST_ID));

        // A later verify of 555 completed → cleanup runs.
        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 1);
        assert!(!install.join("dropped_in_patch.dll").exists());
        assert!(install.join("game.exe").exists());
        assert!(install.join("data.pak").exists());
        assert!(pending_cleanup_markers(&config).is_empty());
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn aborted_build_marker_dropped_when_build_later_commits() {
        let install = temp_dir("aborted_resumed");
        let config = config_dir(&install);
        write_sidecar(&config, 100, 777, &[("game.exe", 0), ("dropped_in_patch.dll", 0)]);
        install_current(&config, 100, 777); // resume finished the update
        touch(&install, "game.exe");
        touch(&install, "dropped_in_patch.dll");
        assert!(record_aborted_build(&config, 100, 777));

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 777)]);

        assert_eq!(deleted, 0);
        assert!(install.join("dropped_in_patch.dll").exists());
        assert!(pending_cleanup_markers(&config).is_empty());
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn marker_for_current_manifest_is_dropped_without_deletions() {
        let install = temp_dir("marker_current");
        let config = config_dir(&install);
        write_manifest(&config, 100, 555, &[("core.dat", 0)]);
        install_current(&config, 100, 555);
        touch(&install, "core.dat");
        fs::write(stale_cleanup_marker_path(&config, 100, 555), "555").unwrap();

        let deleted = run_stale_file_cleanup(install.to_str().unwrap(), &config, &[spec(100, 555)]);

        assert_eq!(deleted, 0);
        assert!(install.join("core.dat").exists());
        assert!(pending_cleanup_markers(&config).is_empty());
        let _ = fs::remove_dir_all(&install);
    }
}
