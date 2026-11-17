// Read-only workspace browsing and git inspection (F1 of
// `odd/tasks/tool-drawer-and-mcp.md`, S2/S3 and the Files/Changes tabs).
//
// Design notes:
// - Read-only by construction: nothing here writes, creates, moves or deletes.
//   The tab browses what the agent is editing; editing stays out of F1.
// - Containment is the core of this module. Every relative path is validated
//   segment by segment (no absolute path, no `..`), then resolved against the
//   canonicalized workspace root and canonicalized itself, so a symlink that
//   points outside the workspace is rejected before a single byte is read.
// - argv only for git: `std::process::Command` with an argument vector and
//   `cwd = workspace`. Never a shell, so a file name is never read as syntax.
// - The parsers are pure and defensive: `git status --porcelain=v1 -z` is
//   parsed from a string in tests, and an unknown status letter is reported as
//   `modified` instead of panicking.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Longest text file the preview decodes. Larger files are reported as
/// `too_large` without content: the drawer shows a tree, not a log viewer.
pub const TEXT_PREVIEW_LIMIT: u64 = 256 * 1024;

/// Largest image the frontend is handed for rendering.
pub const IMAGE_LIMIT: u64 = 8 * 1024 * 1024;

/// How many leading bytes are sniffed for a NUL byte to call a file binary.
pub const BINARY_SNIFF_LEN: usize = 8 * 1024;

/// Longest diff string returned to the webview, in bytes.
pub const DIFF_LIMIT: usize = 512 * 1024;

/// What a directory entry is. Serialized lowercase, so the TS contract is the
/// union `"dir" | "file" | "symlink"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
}

/// One entry of a listed directory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    /// File name only, as the drawer's tree row shows it.
    pub name: String,
    /// Workspace-relative path with `/` separators, the same shape every other
    /// command in this module takes.
    pub path: String,
    pub kind: EntryKind,
    /// Size of the target, in bytes. `0` for directories.
    pub size_bytes: u64,
}

/// Which of the four previews the drawer paints: a decoded text, an image the
/// frontend loads through `fs_read_image_bytes`, a binary it cannot show, or a
/// file too large to preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    Text,
    Image,
    Binary,
    TooLarge,
}

/// Answer of `fs_read_preview`. `content` is only set for `text`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsPreview {
    pub kind: PreviewKind,
    pub size_bytes: u64,
    /// Image mime, read from the extension. `None` for everything else: the
    /// text preview is decoded here and needs no type to render.
    pub mime: Option<String>,
    pub content: Option<String>,
}

/// One file as `git status` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// One row of the Changes tab.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitEntry {
    /// Workspace-relative path with `/` separators. For a rename this is the
    /// destination, the file that exists now and the one a diff addresses.
    pub path: String,
    pub status: GitStatus,
    /// `true` when the change is in the index, `false` when it is only in the
    /// working tree. A file changed in both is reported once, staged.
    pub staged: bool,
}

// ── path containment ────────────────────────────────────────────────────────

/// Label used in error messages: the root is spelled `.`, never an empty string.
fn label(rel_path: &str) -> String {
    let trimmed = rel_path.trim();
    if trimmed.is_empty() || trimmed == "." {
        ".".to_string()
    } else {
        rel_path.to_string()
    }
}

/// Splits a workspace-relative path into segments, rejecting the two things a
/// relative path may never carry: an absolute prefix and a `..` segment.
///
/// Both separators are split on: on Unix a backslash is a legal file name
/// character, but accepting `..\x` unchecked on Windows would be a hole, and a
/// path that means a traversal on any platform is refused on all of them.
fn segments(rel_path: &str) -> Result<Vec<String>, String> {
    let rel = rel_path.trim();
    if rel.is_empty() || rel == "." {
        return Ok(Vec::new());
    }
    if rel.starts_with('/') || rel.starts_with('\\') || Path::new(rel).is_absolute() {
        return Err(format!(
            "path must be workspace-relative, not absolute: {rel}"
        ));
    }
    let mut parts = Vec::new();
    for raw in rel.split(['/', '\\']) {
        if raw.is_empty() || raw == "." {
            continue;
        }
        // Compared trimmed so ` .. ` cannot slip through as a name.
        if raw.trim() == ".." {
            return Err(format!("path escapes the workspace root (`..`): {rel}"));
        }
        parts.push(raw.to_string());
    }
    Ok(parts)
}

/// Canonicalizes the workspace root. Every command starts here, so the root is
/// the one real path all later comparisons use.
fn root_of(workspace: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(workspace);
    if !root.is_absolute() {
        return Err(format!(
            "workspace path must be absolute: {}",
            root.display()
        ));
    }
    let canonical = root
        .canonicalize()
        .map_err(|err| format!("workspace is not readable: {}: {err}", root.display()))?;
    if !canonical.is_dir() {
        return Err(format!(
            "workspace is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

/// The containment rule, in one place: a resolved path must stay inside the
/// canonicalized root. `Path::starts_with` is component-wise, so `/a/bc` is not
/// inside `/a/b`.
fn inside(root: &Path, resolved: &Path, rel_path: &str) -> Result<(), String> {
    if resolved.starts_with(root) {
        return Ok(());
    }
    Err(format!(
        "path resolves outside the workspace root: {}",
        label(rel_path)
    ))
}

/// Resolves an existing target, following symlinks, and proves it stays inside
/// the workspace. Rejects an absolute path, any `..` segment, and a target whose
/// canonical path leaves the root (a symlink pointing outside, for instance).
pub fn resolve(workspace: &str, rel_path: &str) -> Result<PathBuf, String> {
    let root = root_of(workspace)?;
    let parts = segments(rel_path)?;
    let mut candidate = root.clone();
    for part in &parts {
        candidate.push(part);
    }
    let resolved = candidate
        .canonicalize()
        .map_err(|err| format!("cannot read {}: {err}", label(rel_path)))?;
    inside(&root, &resolved, rel_path)?;
    Ok(resolved)
}

/// Containment for a path that may no longer exist (a deleted file has no bytes
/// to canonicalize). The deepest ancestor that does exist is canonicalized and
/// checked, so `link/gone.txt` behind an escaping symlink is still refused.
fn contain_missing(root: &Path, rel_path: &str) -> Result<(), String> {
    let parts = segments(rel_path)?;
    let mut candidate = root.to_path_buf();
    for part in &parts {
        candidate.push(part);
    }
    let mut probe = candidate.as_path();
    loop {
        if let Ok(resolved) = probe.canonicalize() {
            return inside(root, &resolved, rel_path);
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => {
                return Err(format!(
                    "cannot resolve {}: no existing ancestor",
                    label(rel_path)
                ));
            }
        }
    }
}

// ── listings and previews ───────────────────────────────────────────────────

/// The workspace-relative `"a/b/"` prefix of a directory, `""` for the root.
fn prefix_of(rel_path: &str) -> Result<String, String> {
    let parts = segments(rel_path)?;
    if parts.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("{}/", parts.join("/")))
}

/// Whether an entry sorts before the other: directories first, then everything
/// else, each group alphabetical ignoring case, ties broken by the exact name so
/// the order is stable.
fn sort_key(entry: &FsEntry) -> (u8, String, String) {
    let group = if entry.kind == EntryKind::Dir { 0 } else { 1 };
    (group, entry.name.to_lowercase(), entry.name.clone())
}

/// Lists one directory of the workspace. `rel_path` is `""` or `"."` for the
/// root. Entries that cannot be stat-ed are skipped instead of failing the
/// listing: a broken symlink must not hide its siblings.
pub fn list_dir(workspace: &str, rel_path: &str) -> Result<Vec<FsEntry>, String> {
    let dir = resolve(workspace, rel_path)?;
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", label(rel_path)));
    }
    let prefix = prefix_of(rel_path)?;
    let mut entries = Vec::new();
    let reader = std::fs::read_dir(&dir)
        .map_err(|err| format!("cannot read {}: {err}", label(rel_path)))?;
    for entry in reader {
        let Ok(entry) = entry else { continue };
        // `file_type` does not follow the link, so a symlink is reported as
        // one; the metadata does follow it, which is what a size means here.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(entry.path()) else {
            continue;
        };
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink
        } else if metadata.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        let name = entry.file_name().to_string_lossy().to_string();
        push_entry(&mut entries, name, prefix.clone(), kind, metadata);
    }
    entries.sort_by_key(sort_key);
    Ok(entries)
}

/// Pushes one listing row. Directories report `0` bytes: their size on disk is
/// an allocator detail, not something a tree row should show.
fn push_entry(
    entries: &mut Vec<FsEntry>,
    name: String,
    prefix: String,
    kind: EntryKind,
    metadata: std::fs::Metadata,
) {
    let size_bytes = if metadata.is_dir() { 0 } else { metadata.len() };
    entries.push(FsEntry {
        path: format!("{prefix}{name}"),
        name,
        kind,
        size_bytes,
    });
}

/// Mime of the image extensions the drawer renders. Anything else is not an
/// image for this module, whatever the file contains.
pub fn image_mime(name: &str) -> Option<&'static str> {
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Decides how the drawer previews one file, reading at most the bytes it needs:
/// the 8 KiB sniff for the binary check, and the whole file only when it is text
/// small enough to decode.
pub fn preview(workspace: &str, rel_path: &str) -> Result<FsPreview, String> {
    let file = resolve(workspace, rel_path)?;
    if file.is_dir() {
        return Err(format!("not a file (it is a directory): {}", label(rel_path)));
    }
    let size_bytes = std::fs::metadata(&file)
        .map_err(|err| format!("cannot read {}: {err}", label(rel_path)))?
        .len();
    let mime = image_mime(rel_path);
    if let Some(mime) = mime {
        // An image too big to hand over is still an image: the frontend needs
        // the mime to say what it could not show.
        let kind = if size_bytes > IMAGE_LIMIT {
            PreviewKind::TooLarge
        } else {
            PreviewKind::Image
        };
        return Ok(FsPreview {
            kind,
            size_bytes,
            mime: Some(mime.to_string()),
            content: None,
        });
    }
    let mut handle = std::fs::File::open(&file)
        .map_err(|err| format!("cannot read {}: {err}", label(rel_path)))?;
    let mut head = Vec::new();
    handle
        .by_ref()
        .take(BINARY_SNIFF_LEN as u64)
        .read_to_end(&mut head)
        .map_err(|err| format!("cannot read {}: {err}", label(rel_path)))?;
    if head.contains(&0) {
        return Ok(FsPreview {
            kind: PreviewKind::Binary,
            size_bytes,
            mime: None,
            content: None,
        });
    }
    if size_bytes > TEXT_PREVIEW_LIMIT {
        return Ok(FsPreview {
            kind: PreviewKind::TooLarge,
            size_bytes,
            mime: None,
            content: None,
        });
    }
    let mut bytes = head;
    handle
        .read_to_end(&mut bytes)
        .map_err(|err| format!("cannot read {}: {err}", label(rel_path)))?;
    // The file may have grown between the metadata call and the read.
    if bytes.len() as u64 > TEXT_PREVIEW_LIMIT {
        return Ok(FsPreview {
            kind: PreviewKind::TooLarge,
            size_bytes: bytes.len() as u64,
            mime: None,
            content: None,
        });
    }
    Ok(FsPreview {
        kind: PreviewKind::Text,
        size_bytes,
        mime: None,
        // Lossy on purpose: one invalid byte must never fail a preview.
        content: Some(String::from_utf8_lossy(&bytes).into_owned()),
    })
}

/// Raw bytes of one image, for the frontend to build an object URL. Containment
/// and the size cap are the same as `preview`'s.
pub fn image_bytes(workspace: &str, rel_path: &str) -> Result<Vec<u8>, String> {
    let name = label(rel_path);
    // Containment first: an escaping symlink must be reported as such, whatever
    // the extension behind it is.
    let file = resolve(workspace, rel_path)?;
    let mime = image_mime(rel_path)
        .ok_or_else(|| format!("not an image (unknown image extension): {name}"))?;
    if file.is_dir() {
        return Err(format!("not a file (it is a directory): {name}"));
    }
    let size_bytes = std::fs::metadata(&file)
        .map_err(|err| format!("cannot read {name}: {err}"))?
        .len();
    if size_bytes > IMAGE_LIMIT {
        return Err(format!(
            "{name} ({mime}) is over the {} MiB preview limit",
            IMAGE_LIMIT / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(&file).map_err(|err| format!("cannot read {name}: {err}"))?;
    if bytes.len() as u64 > IMAGE_LIMIT {
        return Err(format!(
            "{name} ({mime}) grew over the {} MiB preview limit",
            IMAGE_LIMIT / (1024 * 1024)
        ));
    }
    Ok(bytes)
}

// ── git ─────────────────────────────────────────────────────────────────────

/// Runs one git command. argv only, `cwd = workspace`, and `--no-pager` so a
/// human's pager configuration can never capture the output.
fn run_git(root: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    std::process::Command::new("git")
        .arg("--no-pager")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|err| format!("cannot run git: {err}"))
}

/// Whether the workspace is the top of a git work tree. Nabla shows an empty
/// Changes tab outside a repository instead of an error.
fn is_repo(root: &Path) -> bool {
    matches!(
        run_git(root, &["rev-parse", "--is-inside-work-tree"]),
        Ok(out) if out.status.success()
    )
}

/// The status letter of one side of a porcelain record. An unknown letter is
/// read as `modified`: something changed, and the drawer never hides a file.
fn status_of(letter: char) -> GitStatus {
    match letter {
        'A' | 'C' => GitStatus::Added,
        'D' => GitStatus::Deleted,
        'R' => GitStatus::Renamed,
        '?' => GitStatus::Untracked,
        _ => GitStatus::Modified,
    }
}

/// Reads one `XY path` record of `--porcelain=v1 -z`.
fn entry_of(field: &str) -> Option<GitEntry> {
    let mut letters = field.chars();
    let x = letters.next()?;
    let y = letters.next()?;
    // `XY` is followed by one space; `-z` never quotes the path.
    let rest = field.get(3..)?;
    if rest.is_empty() {
        return None;
    }
    if x == '!' && y == '!' {
        // Ignored, only present with `--ignored`.
        return None;
    }
    // A conflict lives in the index but cannot be committed until it is
    // resolved, so `staged` would mislead: it is always reported unstaged.
    let conflicted = matches!((x, y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D'));
    if conflicted {
        return Some(GitEntry {
            path: rest.to_string(),
            status: GitStatus::Conflicted,
            staged: false,
        });
    }
    if x == '?' {
        return Some(GitEntry {
            path: rest.to_string(),
            status: GitStatus::Untracked,
            staged: false,
        });
    }
    // A file changed in both the index and the worktree (`MM`) is one row:
    // the index side wins, so the row reads as staged.
    let (letter, staged) = if x != ' ' { (x, true) } else { (y, false) };
    Some(GitEntry {
        path: rest.to_string(),
        status: status_of(letter),
        staged,
    })
}

/// Parses `git status --porcelain=v1 -z`. In `-z` a rename carries its
/// destination first and its source as the next field, so the source is consumed
/// and the destination is the row: it is the file that exists and the one a diff
/// addresses.
pub fn parse_status(raw: &str) -> Vec<GitEntry> {
    let mut entries = Vec::new();
    let mut fields = raw.split('\0');
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        if let Some(entry) = entry_of(field) {
            if matches!(entry.status, GitStatus::Renamed) {
                let _source = fields.next();
            }
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

/// The workspace's own path inside the repository, with a trailing slash
/// (`"pkg/"`), empty when the workspace is the repository root. `--porcelain`
/// paths are relative to the repository root, so this is the prefix to strip.
fn repo_prefix(root: &Path) -> String {
    match run_git(root, &["rev-parse", "--show-prefix"]) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => String::new(),
    }
}

/// Rewrites repository-relative paths as workspace-relative ones, dropping the
/// changes that live outside the workspace: they are not browsable here.
fn relative_to_workspace(entries: Vec<GitEntry>, prefix: &str) -> Vec<GitEntry> {
    if prefix.is_empty() {
        return entries;
    }
    entries
        .into_iter()
        .filter_map(|mut entry| {
            entry.path = entry.path.strip_prefix(prefix)?.to_string();
            Some(entry)
        })
        .collect()
}

/// The Changes tab's list. Outside a git repository the answer is an empty list,
/// not an error: browsing a plain folder is legitimate.
pub fn git_status(workspace: &str) -> Result<Vec<GitEntry>, String> {
    let root = root_of(workspace)?;
    if !is_repo(&root) {
        return Ok(Vec::new());
    }
    let out = run_git(&root, &["status", "--porcelain=v1", "-z"])?;
    if !out.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let entries = parse_status(&String::from_utf8_lossy(&out.stdout));
    Ok(relative_to_workspace(entries, &repo_prefix(&root)))
}

/// Caps `text` at `DIFF_LIMIT` bytes, cutting on a character boundary and saying
/// so in the answer: a silently short diff would read as a small change.
pub fn truncate_diff(text: &str) -> String {
    if text.len() <= DIFF_LIMIT {
        return text.to_string();
    }
    let mut cut = DIFF_LIMIT;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!(
        "{}\n\n... diff truncated at {DIFF_LIMIT} bytes ...\n",
        &text[..cut]
    )
}

/// The diff of a file git does not track: its content as an added block.
pub fn added_block(path: &str, text: &str) -> String {
    let mut out = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n"
    );
    for line in text.lines() {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The diff of a file `git status` reports as untracked, built from what the
/// preview decided: text becomes an added block, anything else a one-line note
/// instead of a megabyte of bytes.
pub fn untracked_diff(path: &str, preview: &FsPreview) -> String {
    match preview.kind {
        PreviewKind::Text => added_block(path, preview.content.as_deref().unwrap_or("")),
        _ => {
            let what = match preview.kind {
                PreviewKind::Image => "image",
                PreviewKind::Binary => "binary file",
                _ => "file too large to display",
            };
            format!(
                "diff --git a/{path} b/{path}\nnew file\n{what} ({} bytes), content not shown\n",
                preview.size_bytes
            )
        }
    }
}

/// One file against `HEAD`, or the whole content as an added block when git does
/// not track it yet. The answer is capped by `truncate_diff`.
pub fn git_diff(workspace: &str, path: &str) -> Result<String, String> {
    let root = root_of(workspace)?;
    let parts = segments(path)?;
    if parts.is_empty() {
        return Err("a file path is required to diff".to_string());
    }
    let rel = parts.join("/");
    if !is_repo(&root) {
        return Err(format!(
            "not a git repository: {}",
            root.to_string_lossy()
        ));
    }
    // A deleted file no longer exists, so containment checks the deepest
    // ancestor that does instead of failing on a missing leaf.
    contain_missing(&root, &rel)?;
    let tracked = matches!(
        run_git(&root, &["ls-files", "--error-unmatch", "--", &rel]),
        Ok(out) if out.status.success()
    );
    if !tracked {
        let preview = preview(workspace, &rel)?;
        return Ok(truncate_diff(&untracked_diff(&rel, &preview)));
    }
    let out = run_git(&root, &["diff", "HEAD", "--", &rel])?;
    if out.status.success() {
        return Ok(truncate_diff(&String::from_utf8_lossy(&out.stdout)));
    }
    // A repository without commits has no HEAD to diff against: the index is the
    // only reference left.
    let fallback = run_git(&root, &["diff", "--", &rel])?;
    if fallback.status.success() {
        return Ok(truncate_diff(&String::from_utf8_lossy(&fallback.stdout)));
    }
    Err(format!(
        "git diff failed: {}",
        String::from_utf8_lossy(&fallback.stderr).trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch tree in the system temp dir — never the worktree — removed in
    /// teardown.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "nabla_workspace_{}_{}_{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(path.join("ws")).expect("create temp root");
            TempTree(path)
        }

        /// The workspace the tests browse.
        fn ws(&self) -> PathBuf {
            self.0.join("ws")
        }

        fn ws_str(&self) -> String {
            self.ws().to_string_lossy().to_string()
        }

        fn dir(&self, rel: &str) -> PathBuf {
            let path = self.ws().join(rel);
            std::fs::create_dir_all(&path).expect("create dir");
            path
        }

        fn file(&self, rel: &str, body: &[u8]) -> PathBuf {
            let path = self.ws().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&path, body).expect("write file");
            path
        }

        /// Something outside the workspace, used as a symlink target.
        fn outside(&self, rel: &str, body: &[u8]) -> PathBuf {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create outside parent");
            }
            std::fs::write(&path, body).expect("write outside file");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn symlink(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("create symlink");
    }

    fn names(entries: &[FsEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.path.as_str()).collect()
    }

    // ── security: containment ────────────────────────────────────────────────

    #[test]
    fn list_dir_rejects_parent_traversal() {
        let tree = TempTree::new("list_traversal");
        tree.file("a.txt", b"a");
        tree.outside("secret.txt", b"secret");

        let err = list_dir(&tree.ws_str(), "..").expect_err("`..` must be rejected");
        assert!(err.contains(".."), "message names the rejected segment: {err}");
        let err = list_dir(&tree.ws_str(), "sub/../../").expect_err("nested `..` must be rejected");
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn list_dir_rejects_an_absolute_path() {
        let tree = TempTree::new("list_absolute");
        tree.file("a.txt", b"a");
        let absolute = tree.0.join("secret.txt");
        tree.outside("secret.txt", b"secret");

        let err = list_dir(&tree.ws_str(), &absolute.to_string_lossy())
            .expect_err("an absolute path must be rejected");
        assert!(err.contains("absolute"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn list_dir_rejects_a_symlink_that_escapes_the_workspace() {
        let tree = TempTree::new("list_escape");
        std::fs::create_dir_all(tree.0.join("outside")).expect("outside dir");
        tree.outside("outside/secret.txt", b"secret");
        symlink(&tree.0.join("outside"), &tree.ws().join("escape"));

        let err = list_dir(&tree.ws_str(), "escape").expect_err("escaping symlink must be rejected");
        assert!(err.contains("outside the workspace"), "{err}");
        // The traversal through the link is rejected too, not just the link.
        let err = list_dir(&tree.ws_str(), "escape/secret.txt").expect_err("nested escape");
        assert!(err.contains("outside the workspace"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn list_dir_allows_a_symlink_that_stays_inside() {
        let tree = TempTree::new("list_inside_link");
        tree.file("real/inside.txt", b"inside");
        symlink(&tree.ws().join("real"), &tree.ws().join("link"));

        let entries = list_dir(&tree.ws_str(), "link").expect("an inside symlink must be readable");
        assert_eq!(names(&entries), vec!["link/inside.txt"]);
    }

    #[test]
    fn list_dir_of_the_workspace_root_puts_directories_first_then_files() {
        let tree = TempTree::new("list_order");
        tree.file("zeta.txt", b"z");
        tree.dir("beta");
        tree.file("alpha.txt", b"alpha");
        tree.dir("Alpha");

        // `""` and `"."` are both the root; neither may be rejected.
        let entries = list_dir(&tree.ws_str(), "").expect("root listing");
        assert_eq!(
            names(&entries),
            vec!["Alpha", "beta", "alpha.txt", "zeta.txt"],
            "directories first, then files, each alphabetical ignoring case"
        );
        assert_eq!(list_dir(&tree.ws_str(), ".").expect("dot root"), entries);
        assert_eq!(entries[2].size_bytes, 5, "`alpha.txt` is 5 bytes");
        assert_eq!(entries[2].kind, EntryKind::File);
        assert_eq!(entries[0].kind, EntryKind::Dir);
        assert_eq!(entries[0].name, "Alpha");
    }

    #[cfg(unix)]
    #[test]
    fn list_dir_skips_entries_that_cannot_be_statted() {
        let tree = TempTree::new("list_broken");
        tree.file("real.txt", b"real");
        symlink(&tree.0.join("gone"), &tree.ws().join("broken"));

        let entries = list_dir(&tree.ws_str(), "").expect("listing");
        assert_eq!(names(&entries), vec!["real.txt"], "a broken symlink is skipped");
    }

    // ── security: previews ───────────────────────────────────────────────────

    #[test]
    fn preview_rejects_parent_traversal() {
        let tree = TempTree::new("preview_traversal");
        tree.outside("secret.txt", b"secret");
        tree.file("a.txt", b"a");

        let err = preview(&tree.ws_str(), "../secret.txt").expect_err("`..` must be rejected");
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn preview_rejects_an_absolute_path() {
        let tree = TempTree::new("preview_absolute");
        let absolute = tree.outside("secret.txt", b"secret");
        let err = preview(&tree.ws_str(), &absolute.to_string_lossy()).expect_err("absolute");
        assert!(err.contains("absolute"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn preview_rejects_a_symlink_that_escapes_the_workspace() {
        let tree = TempTree::new("preview_escape");
        let secret = tree.outside("secret.txt", b"secret");
        symlink(&secret, &tree.ws().join("link.txt"));

        let err = preview(&tree.ws_str(), "link.txt").expect_err("escaping symlink");
        assert!(err.contains("outside the workspace"), "{err}");
        // The image path obeys the same rule: no way around it.
        let err = image_bytes(&tree.ws_str(), "link.txt").expect_err("escaping symlink");
        assert!(err.contains("outside the workspace"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn preview_reads_a_symlink_that_stays_inside() {
        let tree = TempTree::new("preview_inside_link");
        tree.file("real.txt", b"inside");
        symlink(&tree.ws().join("real.txt"), &tree.ws().join("link.txt"));

        let preview = preview(&tree.ws_str(), "link.txt").expect("an inside symlink is readable");
        assert_eq!(preview.kind, PreviewKind::Text);
        assert_eq!(preview.content.as_deref(), Some("inside"));
    }

    #[test]
    fn preview_of_the_workspace_root_is_a_readable_error() {
        let tree = TempTree::new("preview_root");
        let err = preview(&tree.ws_str(), "").expect_err("the root is a directory");
        assert!(err.contains("directory"), "{err}");
    }

    #[test]
    fn preview_of_a_missing_file_is_an_error() {
        let tree = TempTree::new("preview_missing");
        let err = preview(&tree.ws_str(), "nope.txt").expect_err("missing file");
        assert!(!err.is_empty());
    }

    // ── previews: kind detection ─────────────────────────────────────────────

    #[test]
    fn preview_decodes_text_lossily_so_one_bad_byte_never_fails() {
        let tree = TempTree::new("preview_text");
        tree.file("ok.txt", b"hola");
        tree.file("bad.txt", b"hola \xff mundo");

        let ok = preview(&tree.ws_str(), "ok.txt").expect("text");
        assert_eq!(ok.kind, PreviewKind::Text);
        assert_eq!(ok.content.as_deref(), Some("hola"));
        assert_eq!(ok.size_bytes, 4);
        assert_eq!(ok.mime, None);

        let bad = preview(&tree.ws_str(), "bad.txt").expect("invalid utf-8 must not fail");
        assert_eq!(bad.kind, PreviewKind::Text);
        assert!(
            bad.content.expect("lossy content").contains('\u{fffd}'),
            "the invalid byte becomes the replacement character"
        );
    }

    #[test]
    fn preview_detects_binary_by_a_nul_byte() {
        let tree = TempTree::new("preview_binary");
        tree.file("nul.bin", b"abc\0def");

        let binary = preview(&tree.ws_str(), "nul.bin").expect("binary");
        assert_eq!(binary.kind, PreviewKind::Binary);
        assert_eq!(binary.size_bytes, 7);
        assert_eq!(binary.content, None);
    }

    #[test]
    fn preview_reports_a_text_file_over_the_limit_without_content() {
        let tree = TempTree::new("preview_too_large");
        let body = vec![b'a'; TEXT_PREVIEW_LIMIT as usize + 1];
        tree.file("big.txt", &body);

        let preview = preview(&tree.ws_str(), "big.txt").expect("too large");
        assert_eq!(preview.kind, PreviewKind::TooLarge);
        assert_eq!(preview.size_bytes, TEXT_PREVIEW_LIMIT + 1);
        assert_eq!(preview.content, None);
    }

    #[test]
    fn preview_reports_an_image_by_extension_without_content() {
        let tree = TempTree::new("preview_image");
        tree.file("shot.PNG", b"\x89PNG\r\n\x1a\n");

        let preview = preview(&tree.ws_str(), "shot.PNG").expect("image");
        assert_eq!(preview.kind, PreviewKind::Image);
        assert_eq!(preview.mime.as_deref(), Some("image/png"));
        assert_eq!(preview.content, None, "bytes go through fs_read_image_bytes");
        assert_eq!(image_mime("x.webp"), Some("image/webp"));
        assert_eq!(image_mime("x.svg"), Some("image/svg+xml"));
        assert_eq!(image_mime("x.woff2"), None);
    }

    #[test]
    fn preview_reports_an_image_over_the_limit_as_too_large_with_its_mime() {
        let tree = TempTree::new("preview_big_image");
        let body = vec![0x89u8; IMAGE_LIMIT as usize + 1];
        tree.file("huge.png", &body);

        let preview = preview(&tree.ws_str(), "huge.png").expect("too large");
        assert_eq!(preview.kind, PreviewKind::TooLarge);
        assert_eq!(preview.size_bytes, IMAGE_LIMIT + 1);
        assert_eq!(preview.mime.as_deref(), Some("image/png"));
        assert_eq!(preview.content, None);
        let err = image_bytes(&tree.ws_str(), "huge.png").expect_err("over the limit");
        assert!(err.contains("limit"), "{err}");
    }

    #[test]
    fn image_bytes_returns_the_file_and_refuses_non_images() {
        let tree = TempTree::new("image_bytes");
        tree.file("ok.gif", b"GIF89a");
        tree.file("note.txt", b"text");

        assert_eq!(
            image_bytes(&tree.ws_str(), "ok.gif").expect("bytes"),
            b"GIF89a"
        );
        let err = image_bytes(&tree.ws_str(), "note.txt").expect_err("not an image");
        assert!(err.contains("not an image"), "{err}");
    }

    // ── git status parsing ───────────────────────────────────────────────────

    #[test]
    fn status_parses_worktree_and_index_changes() {
        // ` M` modified in the worktree, `M ` staged, `A ` added, ` D` deleted.
        let raw = " M src/a.rs\0M  src/b.rs\0A  src/new.rs\0 D gone.rs\0";

        let entries = parse_status(raw);

        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries[0],
            GitEntry {
                path: "gone.rs".into(),
                status: GitStatus::Deleted,
                staged: false
            }
        );
        assert_eq!(entries[1].path, "src/a.rs");
        assert_eq!(entries[1].status, GitStatus::Modified);
        assert!(!entries[1].staged, "` M` is a worktree change");
        assert_eq!(entries[2].path, "src/b.rs");
        assert_eq!(entries[2].status, GitStatus::Modified);
        assert!(entries[2].staged, "`M ` is an index change");
        assert_eq!(entries[3].path, "src/new.rs");
        assert_eq!(entries[3].status, GitStatus::Added);
        assert!(entries[3].staged);
    }

    #[test]
    fn status_parses_a_rename_with_its_source_field_consumed() {
        // `-z` gives the destination first, then the source as its own field.
        let raw = "R  src/new_name.rs\0src/old_name.rs\0 M other.rs\0";

        let entries = parse_status(raw);

        assert_eq!(entries.len(), 2, "the source is not a row of its own");
        assert_eq!(entries[0].path, "other.rs");
        assert_eq!(entries[0].status, GitStatus::Modified);
        assert_eq!(entries[1].path, "src/new_name.rs");
        assert_eq!(entries[1].status, GitStatus::Renamed);
        assert!(entries[1].staged);
    }

    #[test]
    fn status_parses_untracked_and_conflicted() {
        let raw = "?? notes.md\0UU merge.rs\0AA both.rs\0";

        let entries = parse_status(raw);

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "both.rs");
        assert_eq!(entries[0].status, GitStatus::Conflicted);
        assert_eq!(entries[1].path, "merge.rs");
        assert_eq!(entries[1].status, GitStatus::Conflicted);
        assert!(!entries[1].staged);
        assert_eq!(entries[2].path, "notes.md");
        assert_eq!(entries[2].status, GitStatus::Untracked);
        assert!(!entries[2].staged);
    }

    #[test]
    fn status_ignores_a_trailing_empty_field_and_unknown_letters() {
        assert!(parse_status("").is_empty());
        assert!(parse_status("\0\0").is_empty(), "no entry is invented");

        let entries = parse_status(" M weird.rs\0");
        assert_eq!(entries.len(), 1, "a short record is not dropped");
        assert_eq!(entries[0].status, GitStatus::Modified);
    }

    #[test]
    fn status_is_empty_outside_a_git_repository() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let tree = TempTree::new("status_nongit");
        tree.file("a.txt", b"a");
        assert_eq!(git_status(&tree.ws_str()).expect("not an error"), Vec::new());
    }

    #[test]
    fn status_reads_a_real_repository() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let tree = TempTree::new("status_repo");
        init_repo(&tree);
        tree.file("tracked.txt", b"one\n");
        commit_all(&tree, "first");
        tree.file("tracked.txt", b"one\ntwo\n");
        tree.file("fresh.txt", b"new\n");

        let entries = git_status(&tree.ws_str()).expect("status");

        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["fresh.txt", "tracked.txt"], "sorted by path");
        assert_eq!(entries[0].status, GitStatus::Untracked);
        assert_eq!(entries[1].status, GitStatus::Modified);
        assert!(!entries[1].staged);
    }

    #[test]
    fn status_is_workspace_relative_when_the_workspace_is_a_subdirectory() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        // `--porcelain` paths are relative to the repository root, not to the
        // directory git runs in, so a workspace inside a repo must strip the
        // prefix or the UI would show paths its own commands cannot open.
        let tree = TempTree::new("status_subdir");
        init_repo(&tree);
        tree.file("pkg/tracked.txt", b"one\n");
        commit_all(&tree, "first");
        tree.file("pkg/tracked.txt", b"one\ntwo\n");
        tree.file("pkg/fresh.txt", b"new\n");

        let nested = tree.ws().join("pkg");
        let entries = git_status(&nested.to_string_lossy()).expect("status");

        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["fresh.txt", "tracked.txt"], "workspace-relative");
        // And the reported path is the one the other commands accept.
        let diff = git_diff(&nested.to_string_lossy(), "tracked.txt").expect("diff");
        assert!(diff.contains("+two"), "{diff}");
    }

    // ── git diff ─────────────────────────────────────────────────────────────

    #[test]
    fn diff_truncation_helper_caps_the_output_and_says_so() {
        let small = "small diff\n";
        assert_eq!(truncate_diff(small), small, "no marker when nothing is cut");

        let exact = "a".repeat(DIFF_LIMIT);
        assert_eq!(truncate_diff(&exact).len(), DIFF_LIMIT, "the cap is inclusive");

        let big = "a".repeat(DIFF_LIMIT + 10);
        let cut = truncate_diff(&big);
        assert!(cut.starts_with(&"a".repeat(64)), "the head survives");
        assert!(
            cut.contains("truncated"),
            "the string names the truncation: {cut}"
        );
        assert!(cut.len() < big.len() + 128, "the cap still bounds the answer");
    }

    #[test]
    fn diff_added_block_marks_every_line_as_added() {
        let block = added_block("notes/new.md", "line one\nline two\n");

        assert!(block.contains("diff --git a/notes/new.md b/notes/new.md"));
        assert!(block.contains("--- /dev/null"), "{block}");
        assert!(block.contains("+++ b/notes/new.md"));
        assert!(block.contains("\n+line one\n"), "{block}");
        assert!(block.contains("\n+line two"), "{block}");
    }

    #[test]
    fn diff_of_an_untracked_file_is_its_content_as_an_added_block() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let tree = TempTree::new("diff_untracked");
        init_repo(&tree);
        tree.file("fresh.txt", b"hello\n");

        let diff = git_diff(&tree.ws_str(), "fresh.txt").expect("diff");

        assert!(diff.contains("+++ b/fresh.txt"), "{diff}");
        assert!(diff.contains("\n+hello"), "{diff}");
    }

    #[test]
    fn diff_of_a_tracked_change_shows_the_hunk() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let tree = TempTree::new("diff_tracked");
        init_repo(&tree);
        tree.file("tracked.txt", b"one\n");
        commit_all(&tree, "first");
        tree.file("tracked.txt", b"one\ntwo\n");

        let diff = git_diff(&tree.ws_str(), "tracked.txt").expect("diff");

        assert!(diff.contains("diff --git a/tracked.txt b/tracked.txt"), "{diff}");
        assert!(diff.contains("+two"), "{diff}");
    }

    #[test]
    fn diff_rejects_a_path_outside_the_workspace() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let tree = TempTree::new("diff_escape");
        init_repo(&tree);
        tree.file("a.txt", b"a");
        tree.outside("secret.txt", b"secret");

        let err = git_diff(&tree.ws_str(), "../secret.txt").expect_err("traversal");
        assert!(err.contains(".."), "{err}");
    }

    // ── git helpers of the tests ─────────────────────────────────────────────

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    fn git(tree: &TempTree, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .args(args)
            .current_dir(tree.ws())
            .output()
            .expect("run git")
    }

    fn init_repo(tree: &TempTree) {
        let out = git(tree, &["init", "-q", "-b", "main"]);
        assert!(out.status.success(), "git init failed");
    }

    fn commit_all(tree: &TempTree, message: &str) {
        assert!(git(tree, &["add", "."]).status.success(), "git add");
        let out = git(
            tree,
            &[
                "-c",
                "user.email=test@nabla.invalid",
                "-c",
                "user.name=Nabla Test",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
        assert!(out.status.success(), "git commit failed");
    }
}
