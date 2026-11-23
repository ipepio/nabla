//! Bundled `pi` runtime resolution (`@earendil-works/pi-coding-agent` + portable Node).
//!
//! Resolution order when Nabla spawns `pi` (RPC sessions, CLI commands, probes):
//!
//! 1. If `settings.pi_binary` is a custom path (anything other than `"pi"` or empty),
//!    honor that override verbatim.
//! 2. Otherwise, check the embedded runtime bundled into standalone installers:
//!    `<resource_dir>/pi-runtime/bin/node(.exe)` (preferred — keeping Node under
//!    `Resources/` instead of macOS `Contents/MacOS/` prevents LaunchServices
//!    from spawning a duplicate `exec` Dock icon for every pi session) or the
//!    legacy sidecar `<exe_dir>/nabla-node(.exe)`, together with
//!    `<resource_dir>/pi-runtime/node_modules/@earendil-works/pi-coding-agent/dist/cli.js`.
//!    When both exist, Nabla invokes `<node> <cli.js> ...` with `NODE_PATH`
//!    pointing at `<resource_dir>/pi-runtime/node_modules` so extensions like
//!    `nabla-subagents.ts` resolve `@earendil-works/pi-coding-agent` and `typebox`
//!    with zero external Node or `pi` installation on the user's machine.
//! 3. Otherwise (e.g. local `pnpm tauri dev` without a prepared bundle), fall back
//!    to `pi` on `PATH`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();
static AGENT_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Records Tauri's resolved `resource_dir()` once at app startup.
pub fn init_resource_dir(dir: PathBuf) {
    let _ = RESOURCE_DIR.set(dir);
}

/// Nabla's isolated pi agent directory (`<data_dir>/pi-agent`), passed via
/// `PI_CODING_AGENT_DIR` so Nabla never inherits or mutates `~/.pi/agent/`.
pub fn default_agent_dir(data_dir: &Path) -> PathBuf {
    strip_windows_verbatim_prefix(&data_dir.join("pi-agent"))
}

/// Records Nabla's isolated `PI_CODING_AGENT_DIR` once at app startup.
pub fn init_agent_dir(dir: PathBuf) {
    let _ = AGENT_DIR.set(strip_windows_verbatim_prefix(&dir));
}

/// Returns Nabla's isolated `PI_CODING_AGENT_DIR` when initialized.
pub fn agent_dir() -> Option<&'static Path> {
    AGENT_DIR.get().map(PathBuf::as_path)
}

/// How to spawn `pi`: the executable, any leading arguments (e.g. `cli.js`), and
/// an optional `NODE_PATH` so runtime extensions can import pi's packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPi {
    pub program: PathBuf,
    pub prefix_args: Vec<String>,
    pub node_path: Option<PathBuf>,
}

const PI_CLI_REL: &[&str] = &[
    "pi-runtime",
    "node_modules",
    "@earendil-works",
    "pi-coding-agent",
    "dist",
    "cli.js",
];

const PI_NODE_MODULES_REL: &[&str] = &["pi-runtime", "node_modules"];

fn bundled_node_name() -> &'static str {
    if cfg!(windows) {
        "node.exe"
    } else {
        "node"
    }
}

fn sidecar_node_name() -> &'static str {
    if cfg!(windows) {
        "nabla-node.exe"
    } else {
        "nabla-node"
    }
}

/// Strips Windows verbatim path prefixes (`\\?\` and `\\?\UNC\`) that Tauri's
/// `resource_dir()` / `canonicalize()` produce. Node.js 22's `realpathSync`
/// crashes with `EISDIR: illegal operation on a directory, lstat 'C:'` when
/// its main script path starts with `\\?\C:\...`.
fn strip_windows_verbatim_prefix(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

fn join_parts(base: &Path, parts: &[&str]) -> PathBuf {
    let mut out = base.to_path_buf();
    for part in parts {
        out.push(part);
    }
    out
}

/// Pure resolution helper (injectable paths for unit tests).
pub fn resolve_pi_with_paths(
    configured_binary: &str,
    exe_dir: Option<&Path>,
    resource_dir: Option<&Path>,
) -> ResolvedPi {
    let trimmed = configured_binary.trim();
    if !trimmed.is_empty() && trimmed != "pi" {
        return ResolvedPi {
            program: PathBuf::from(trimmed),
            prefix_args: Vec::new(),
            node_path: None,
        };
    }

    let mut candidate_roots: Vec<PathBuf> = Vec::new();
    if let Some(res) = resource_dir {
        candidate_roots.push(res.join("resources"));
        candidate_roots.push(res.to_path_buf());
    }
    if let Some(exe) = exe_dir {
        candidate_roots.push(exe.join("resources"));
        candidate_roots.push(exe.to_path_buf());
        if let Some(parent) = exe.parent() {
            candidate_roots.push(parent.join("Resources").join("resources"));
            candidate_roots.push(parent.join("Resources"));
        }
    }

    let legacy_sidecar = exe_dir
        .map(|exe| exe.join(sidecar_node_name()))
        .filter(|p| p.is_file());

    for root in candidate_roots {
        let cli_js = join_parts(&root, PI_CLI_REL);
        if !cli_js.is_file() {
            continue;
        }
        let resource_node = root.join("pi-runtime").join("bin").join(bundled_node_name());
        let node_bin = if resource_node.is_file() {
            Some(resource_node)
        } else {
            legacy_sidecar.clone()
        };
        if let Some(program) = node_bin {
            let node_modules = join_parts(&root, PI_NODE_MODULES_REL);
            let clean_program = strip_windows_verbatim_prefix(&program);
            let clean_cli_js = strip_windows_verbatim_prefix(&cli_js);
            let clean_node_modules = node_modules
                .is_dir()
                .then(|| strip_windows_verbatim_prefix(&node_modules));
            return ResolvedPi {
                program: clean_program,
                prefix_args: vec![clean_cli_js.to_string_lossy().to_string()],
                node_path: clean_node_modules,
            };
        }
    }

    ResolvedPi {
        program: PathBuf::from("pi"),
        prefix_args: Vec::new(),
        node_path: None,
    }
}

/// Resolves the `pi` invocation for the current running Nabla process.
pub fn resolve_pi(configured_binary: &str) -> ResolvedPi {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let res_dir = RESOURCE_DIR.get().map(PathBuf::as_path);
    resolve_pi_with_paths(configured_binary, exe_dir.as_deref(), res_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nabla_pi_runtime_{}_{tag}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn custom_binary_override_bypasses_bundled_runtime() {
        let dir = temp_dir("override");
        let resolved = resolve_pi_with_paths("/custom/bin/pi", Some(&dir), Some(&dir));
        assert_eq!(resolved.program, PathBuf::from("/custom/bin/pi"));
        assert!(resolved.prefix_args.is_empty());
        assert!(resolved.node_path.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundled_resource_node_preferred_over_legacy_sidecar() {
        let dir = temp_dir("resource_node");
        let exe_dir = dir.join("exe");
        let res_dir = dir.join("res");
        fs::create_dir_all(&exe_dir).unwrap();
        let legacy_node = exe_dir.join(sidecar_node_name());
        fs::write(&legacy_node, b"legacy").unwrap();

        let resource_node = res_dir
            .join("resources")
            .join("pi-runtime")
            .join("bin")
            .join(bundled_node_name());
        fs::create_dir_all(resource_node.parent().unwrap()).unwrap();
        fs::write(&resource_node, b"resource-node").unwrap();

        let cli_js = join_parts(&res_dir.join("resources"), PI_CLI_REL);
        fs::create_dir_all(cli_js.parent().unwrap()).unwrap();
        fs::write(&cli_js, b"// stub cli").unwrap();

        let resolved = resolve_pi_with_paths("pi", Some(&exe_dir), Some(&res_dir));
        assert_eq!(resolved.program, resource_node);
        assert_eq!(
            resolved.prefix_args,
            vec![cli_js.to_string_lossy().to_string()]
        );
        assert_eq!(
            resolved.node_path,
            Some(join_parts(&res_dir.join("resources"), PI_NODE_MODULES_REL))
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundled_runtime_used_when_node_and_cli_exist() {
        let dir = temp_dir("bundled");
        let exe_dir = dir.join("exe");
        let res_dir = dir.join("res");
        fs::create_dir_all(&exe_dir).unwrap();
        let node_bin = exe_dir.join(sidecar_node_name());
        fs::write(&node_bin, b"stub-node").unwrap();

        let cli_js = join_parts(&res_dir.join("resources"), PI_CLI_REL);
        fs::create_dir_all(cli_js.parent().unwrap()).unwrap();
        fs::write(&cli_js, b"// stub cli").unwrap();

        let resolved = resolve_pi_with_paths("pi", Some(&exe_dir), Some(&res_dir));
        assert_eq!(resolved.program, node_bin);
        assert_eq!(resolved.prefix_args, vec![cli_js.to_string_lossy().to_string()]);
        assert_eq!(
            resolved.node_path,
            Some(join_parts(&res_dir.join("resources"), PI_NODE_MODULES_REL))
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn strips_windows_verbatim_prefixes_for_node() {
        assert_eq!(
            strip_windows_verbatim_prefix(Path::new(r"\\?\C:\Users\me\AppData\Local\Nabla\cli.js")),
            PathBuf::from(r"C:\Users\me\AppData\Local\Nabla\cli.js")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(Path::new(r"\\?\UNC\server\share\Nabla\cli.js")),
            PathBuf::from(r"\\server\share\Nabla\cli.js")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(Path::new("/Applications/Nabla.app/Contents/Resources")),
            PathBuf::from("/Applications/Nabla.app/Contents/Resources")
        );
    }

    #[test]
    fn falls_back_to_path_pi_when_bundle_absent() {
        let dir = temp_dir("fallback");
        let resolved = resolve_pi_with_paths("pi", Some(&dir), Some(&dir));
        assert_eq!(resolved.program, PathBuf::from("pi"));
        assert!(resolved.prefix_args.is_empty());
        assert!(resolved.node_path.is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
