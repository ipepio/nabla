//! Composer PDF input (S3 of `odd/tasks/composer-tools-pdf.md`).
//!
//! The rule this module protects: the text the composer sends is text the document
//! really holds, page by page, and nothing about a scan is invented.
//!
//! 1. **Extraction is local and argv-only.** `pdftotext` (poppler) runs as a child
//!    with an argument list — never a shell string — so a file name can never become
//!    a command. The tool is a documented system dependency: when it is missing the
//!    error says exactly how to install it.
//! 2. **Everything is bounded before and during the child.** The file must be a
//!    regular file under [`MAX_PDF_BYTES`] whose bytes start with `%PDF-`; the child
//!    is killed at [`PDF_TIMEOUT`], its stdout is read up to a byte cap, and the
//!    answer is refused over [`MAX_PDF_PAGES`] pages or [`MAX_PDF_TEXT_CHARS`]
//!    characters.
//! 3. **Page numbering is the document's.** `pdftotext` separates pages with a form
//!    feed, so splitting on it answers page N at index N-1 — an empty page at page 2
//!    is `""` at index 1, never a shift. A document with no selectable text at all
//!    answers `has_text: false`; no OCR is attempted and no text is made up.
//!
//! The extracted text is handed to the webview and lives only in the draft
//! (`src/lib/composer-attachments.ts`). It never reaches the database, the settings
//! file or a log line, and the PDF's bytes never cross the bridge at all.
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

/// The executable this module runs. Resolved by name from `PATH`, like `pi`.
pub const PDFTOTEXT: &str = "pdftotext";

/// Pages one PDF may have. A long report is fine; a 1000-page manual is a mistake
/// that would flood the prompt.
pub const MAX_PDF_PAGES: usize = 200;

/// Characters of extracted text one PDF may carry.
pub const MAX_PDF_TEXT_CHARS: usize = 1_000_000;

/// Largest PDF file accepted, in bytes (32 MiB). Checked from metadata, before the
/// document is opened.
pub const MAX_PDF_BYTES: usize = 32 * 1024 * 1024;

/// How long one extraction may take before the child is killed.
pub const PDF_TIMEOUT: Duration = Duration::from_secs(20);

/// One PDF the composer extracted, page by page. Mirrors `PdfAttachment` in
/// `src/contracts/types.ts` field for field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfAttachment {
    /// File name, shown as the attachment's label.
    pub name: String,
    /// Pages the document really has; equals `pages.len()`.
    pub page_count: usize,
    /// Selectable text per page, in page order; `""` for a page that has none.
    pub pages: Vec<String>,
    /// Total characters of `pages`.
    pub char_count: usize,
    /// `false` when no page carries selectable text (a scan, no OCR).
    pub has_text: bool,
    /// Size of the file in bytes.
    pub byte_length: usize,
}

/// The bounds of one extraction. Overridable so tests can run small and fast, and
/// so the production values above stay one definition.
#[derive(Debug, Clone, Copy)]
pub struct PdfLimits {
    pub bytes: usize,
    pub pages: usize,
    pub chars: usize,
    pub timeout: Duration,
}

impl Default for PdfLimits {
    fn default() -> Self {
        PdfLimits {
            bytes: MAX_PDF_BYTES,
            pages: MAX_PDF_PAGES,
            chars: MAX_PDF_TEXT_CHARS,
            timeout: PDF_TIMEOUT,
        }
    }
}

/// Why a PDF was refused. Every variant says what happened and, where it is
/// knowable, what would fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfInputError {
    /// The path is not a regular file (a directory, or something gone).
    NotAFile(String),
    /// The file could not be read.
    Io(String),
    /// The bytes do not start with the PDF header.
    NotAPdf(String),
    /// The file is over the size ceiling.
    TooLarge { bytes: usize, limit: usize },
    /// The document has more pages than one turn accepts.
    TooManyPages { pages: usize, limit: usize },
    /// The extracted text is over the ceiling.
    TooMuchText { chars: usize, limit: usize },
    /// The child was still running at the deadline and was killed.
    TimedOut { seconds: u64 },
    /// `pdftotext` is not installed (or not executable): the message names poppler.
    MissingTool(String),
    /// `pdftotext` ran and refused the document.
    ExtractFailed(String),
}

impl fmt::Display for PdfInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfInputError::NotAFile(path) => write!(formatter, "not a file: {path}"),
            PdfInputError::Io(reason) => write!(formatter, "could not read the PDF: {reason}"),
            PdfInputError::NotAPdf(path) => write!(
                formatter,
                "not a PDF (the file does not start with %PDF-): {path}"
            ),
            PdfInputError::TooLarge { bytes, limit } => write!(
                formatter,
                "the PDF is {bytes} bytes, over the {limit}-byte limit"
            ),
            PdfInputError::TooManyPages { pages, limit } => write!(
                formatter,
                "the PDF has {pages} pages, over the {limit}-page limit"
            ),
            PdfInputError::TooMuchText { chars, limit } => write!(
                formatter,
                "the PDF has {chars} characters of text, over the {limit} limit"
            ),
            PdfInputError::TimedOut { seconds } => write!(
                formatter,
                "the PDF took longer than {seconds}s to extract and was stopped"
            ),
            PdfInputError::MissingTool(tool) => write!(
                formatter,
                "{tool} is not installed: install poppler (brew install poppler, apt install poppler-utils, pacman -S poppler) and try again"
            ),
            PdfInputError::ExtractFailed(reason) => {
                write!(formatter, "pdftotext could not read the PDF: {reason}")
            }
        }
    }
}

impl std::error::Error for PdfInputError {}

/// Whether the first bytes are a PDF header. The extension never decides.
pub fn is_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// Splits `pdftotext` output into pages. The form feed is the page separator, so an
/// empty page keeps its position; the trailing separator poppler writes after the
/// last page is not a page.
pub fn split_pages(text: &str) -> Vec<String> {
    let mut pages: Vec<String> = text
        .split('\u{0c}')
        .map(|page| page.trim_end().to_string())
        .collect();
    // poppler ends the document with a form feed; that empty segment is no page.
    if pages.len() > 1 && pages.last().is_some_and(|page| page.is_empty()) {
        pages.pop();
    }
    if pages.is_empty() {
        pages.push(String::new());
    }
    pages
}

/// Extracts one PDF's selectable text, page by page, bounded end to end.
pub fn read_pdf_attachment(path: &Path) -> Result<PdfAttachment, PdfInputError> {
    read_pdf_attachment_with(Path::new(PDFTOTEXT), path, PdfLimits::default())
}

/// The body of [`read_pdf_attachment`], split out so tests can point at their own
/// executable (a missing one, a slow one, one that records its argv) and use small
/// bounds.
pub fn read_pdf_attachment_with(
    executable: &Path,
    path: &Path,
    limits: PdfLimits,
) -> Result<PdfAttachment, PdfInputError> {
    let display = path.display().to_string();
    let metadata = std::fs::metadata(path).map_err(|_| PdfInputError::NotAFile(display.clone()))?;
    if !metadata.is_file() {
        return Err(PdfInputError::NotAFile(display));
    }
    let byte_length = metadata.len() as usize;
    if byte_length > limits.bytes {
        return Err(PdfInputError::TooLarge {
            bytes: byte_length,
            limit: limits.bytes,
        });
    }
    // The header is checked before the child runs: a renamed text file is refused
    // here, not by poppler's stderr.
    let mut header = [0u8; 8];
    let mut file =
        std::fs::File::open(path).map_err(|error| PdfInputError::Io(error.to_string()))?;
    let read = file
        .read(&mut header)
        .map_err(|error| PdfInputError::Io(error.to_string()))?;
    if !is_pdf(&header[..read]) {
        return Err(PdfInputError::NotAPdf(display));
    }
    drop(file);

    let text = extract_text(executable, path, &limits)?;
    let pages = split_pages(&text);
    if pages.len() > limits.pages {
        return Err(PdfInputError::TooManyPages {
            pages: pages.len(),
            limit: limits.pages,
        });
    }
    let char_count: usize = pages.iter().map(|page| page.chars().count()).sum();
    if char_count > limits.chars {
        return Err(PdfInputError::TooMuchText {
            chars: char_count,
            limit: limits.chars,
        });
    }
    let has_text = pages.iter().any(|page| !page.trim().is_empty());
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "document.pdf".to_string());
    Ok(PdfAttachment {
        name,
        page_count: pages.len(),
        pages,
        char_count,
        has_text,
        byte_length,
    })
}

/// Runs `pdftotext` as an argv-only child and returns its stdout, bounded in time
/// and size. A child that outlives [`PdfLimits::timeout`] is killed and reported as
/// a timeout instead of hanging the app.
fn extract_text(
    executable: &Path,
    path: &Path,
    limits: &PdfLimits,
) -> Result<String, PdfInputError> {
    let mut child = match Command::new(executable)
        .arg("-q")
        .arg("-enc")
        .arg("UTF-8")
        .arg("-eol")
        .arg("unix")
        .arg(path)
        // `-` is pdftotext's own "write the text to stdout".
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(PdfInputError::MissingTool(executable.display().to_string()));
        }
        Err(error) => return Err(PdfInputError::Io(error.to_string())),
    };

    // UTF-8 is at most four bytes per character, so this cap is what makes the
    // character ceiling enforceable while the output is still arriving.
    let byte_cap = limits.chars.saturating_mul(4).saturating_add(1);
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PdfInputError::Io("pdftotext has no stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| PdfInputError::Io("pdftotext has no stderr".to_string()))?;
    let output_reader = std::thread::spawn(move || read_capped(stdout, byte_cap));
    let error_reader = std::thread::spawn(move || read_capped(stderr, 4096));

    let deadline = Instant::now() + limits.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // The readers are detached, not joined: a child that spawned its
                    // own children can leave the pipe open, and the deadline has to
                    // hold for the app, not for a grandchild. Both reads are bounded
                    // by their cap and end on their own when the pipe closes.
                    drop(output_reader);
                    drop(error_reader);
                    return Err(PdfInputError::TimedOut {
                        seconds: limits.timeout.as_secs(),
                    });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                drop(output_reader);
                drop(error_reader);
                return Err(PdfInputError::Io(error.to_string()));
            }
        }
    };

    let (output, truncated) = output_reader.join().unwrap_or_default();
    let (errors, _) = error_reader.join().unwrap_or_default();
    if truncated {
        return Err(PdfInputError::TooMuchText {
            chars: limits.chars.saturating_add(1),
            limit: limits.chars,
        });
    }
    if !status.success() {
        let reason = String::from_utf8_lossy(&errors).trim().to_string();
        let reason = if reason.is_empty() {
            format!("pdftotext exited with {status}")
        } else {
            reason
        };
        return Err(PdfInputError::ExtractFailed(reason));
    }
    Ok(String::from_utf8_lossy(&output).to_string())
}

/// Reads at most `cap` bytes, answering whether the stream held more. `cap + 1` is
/// read on purpose: the extra byte is how "over the ceiling" is observed without
/// buffering the rest.
fn read_capped<R: Read>(stream: R, cap: usize) -> (Vec<u8>, bool) {
    let mut buffer = Vec::new();
    let mut limited = stream.take(cap.saturating_add(1) as u64);
    let _ = limited.read_to_end(&mut buffer);
    let truncated = buffer.len() > cap;
    if truncated {
        buffer.truncate(cap);
    }
    (buffer, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    /// A scratch directory in the system temp dir, removed on drop: no fixture
    /// ever lives in the worktree.
    struct TempPdfs {
        dir: PathBuf,
    }

    impl TempPdfs {
        fn new(tag: &str) -> TempPdfs {
            let mut dir = std::env::temp_dir();
            dir.push(format!(
                "nabla_pdfs_{}_{}_{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempPdfs { dir }
        }

        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.dir.join(name);
            std::fs::write(&path, bytes).expect("write fixture");
            path
        }

        #[cfg(unix)]
        fn script(&self, name: &str, body: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = self.dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod script");
            path
        }
    }

    impl Drop for TempPdfs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Whether the real `pdftotext` is available on this machine. The tests that
    /// need real extraction skip (with a note) when poppler is not installed, so the
    /// suite stays runnable on a host that has the app's optional dependency missing.
    fn pdftotext_available() -> bool {
        Command::new(PDFTOTEXT)
            .arg("-v")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// A minimal, valid PDF (one page per string, standard font) built here so the
    /// fixture is deterministic and lives nowhere in the worktree.
    fn build_pdf(pages: &[&str]) -> Vec<u8> {
        let mut objects: Vec<String> = Vec::new();
        objects.push("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string());
        let kids: Vec<String> = (0..pages.len())
            .map(|index| format!("{} 0 R", 5 + 2 * index))
            .collect();
        objects.push(format!(
            "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
            kids.join(" "),
            pages.len()
        ));
        objects.push(
            "3 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n".to_string(),
        );
        for (index, page) in pages.iter().enumerate() {
            let content = page_ops(page);
            objects.push(format!(
                "{} 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n",
                4 + 2 * index,
                content.len(),
                content
            ));
            objects.push(format!(
                "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>\nendobj\n",
                5 + 2 * index,
                4 + 2 * index
            ));
        }
        let header = "%PDF-1.4\n";
        let mut body = String::new();
        let mut offsets: Vec<usize> = Vec::new();
        let mut position = header.len();
        for object in &objects {
            offsets.push(position);
            body.push_str(object);
            position += object.len();
        }
        let xref_position = position;
        let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
        for offset in offsets {
            xref.push_str(&format!("{offset:010} 00000 n \n"));
        }
        let trailer = format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_position}\n%%EOF\n",
            objects.len() + 1
        );
        format!("{header}{body}{xref}{trailer}").into_bytes()
    }

    /// The text operators of one page, one line per line of text.
    fn page_ops(text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let mut operators: Vec<String> = Vec::new();
        let mut y = 700;
        for line in text.split('\n') {
            let escaped = line
                .replace('\\', "\\\\")
                .replace('(', "\\(")
                .replace(')', "\\)");
            operators.push(format!("BT /F1 14 Tf 72 {y} Td ({escaped}) Tj ET"));
            y -= 20;
        }
        operators.join("\n")
    }

    // S3: page N is index N-1 and an empty page keeps its number instead of
    // shifting the pages after it.
    #[test]
    fn a_page_keeps_its_number_even_when_it_holds_nothing() {
        if !pdftotext_available() {
            eprintln!("skipping: pdftotext is not installed");
            return;
        }
        let pdfs = TempPdfs::new("pages");
        let path = pdfs.write(
            "informe.pdf",
            &build_pdf(&["Alpha page one", "", "Gamma page three"]),
        );
        let attachment = read_pdf_attachment(&path).expect("extract the pdf");
        assert_eq!(attachment.page_count, 3, "three pages, one of them empty");
        assert_eq!(attachment.pages.len(), 3);
        assert_eq!(attachment.pages[0], "Alpha page one");
        assert_eq!(attachment.pages[1], "", "the empty page is page 2");
        assert_eq!(attachment.pages[2], "Gamma page three");
        assert!(attachment.has_text);
        assert_eq!(
            attachment.byte_length,
            std::fs::metadata(&path).unwrap().len() as usize
        );
        assert_eq!(attachment.name, "informe.pdf");
    }

    // S3: a document with no selectable text is reported as a scan — no page is
    // invented, and `has_text` is the honest answer.
    #[test]
    fn a_scan_reports_no_text_instead_of_inventing_it() {
        if !pdftotext_available() {
            eprintln!("skipping: pdftotext is not installed");
            return;
        }
        let pdfs = TempPdfs::new("scan");
        let path = pdfs.write("escaneado.pdf", &build_pdf(&["", ""]));
        let attachment = read_pdf_attachment(&path).expect("extract the pdf");
        assert_eq!(attachment.page_count, 2);
        assert_eq!(attachment.pages, vec![String::new(), String::new()]);
        assert_eq!(attachment.char_count, 0);
        assert!(!attachment.has_text, "a scan has no selectable text");
    }

    #[test]
    fn a_file_that_is_not_a_pdf_is_refused_before_extraction() {
        let pdfs = TempPdfs::new("notpdf");
        let path = pdfs.write("not-really.pdf", b"hello, I am text");
        assert_eq!(
            read_pdf_attachment(&path),
            Err(PdfInputError::NotAPdf(path.display().to_string()))
        );
        assert_eq!(
            read_pdf_attachment(&pdfs.dir.join("gone.pdf")),
            Err(PdfInputError::NotAFile(
                pdfs.dir.join("gone.pdf").display().to_string()
            ))
        );
        let directory = pdfs.dir.join("folder.pdf");
        std::fs::create_dir(&directory).expect("create directory");
        assert!(matches!(
            read_pdf_attachment(&directory),
            Err(PdfInputError::NotAFile(_))
        ));
    }

    // S3: the size check happens from metadata, so an oversized document is never
    // read and never reaches the child.
    #[test]
    fn an_oversized_file_is_refused_from_its_metadata() {
        let pdfs = TempPdfs::new("oversized");
        let path = pdfs.write("huge.pdf", b"%PDF-1.4\n");
        let handle = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open for resize");
        handle
            .set_len((MAX_PDF_BYTES + 1) as u64)
            .expect("grow the file");
        drop(handle);
        assert_eq!(
            read_pdf_attachment(&path),
            Err(PdfInputError::TooLarge {
                bytes: MAX_PDF_BYTES + 1,
                limit: MAX_PDF_BYTES,
            })
        );
    }

    // S3: a missing dependency is a usable message, not a bare spawn error.
    #[test]
    fn a_missing_pdftotext_names_the_package_to_install() {
        let pdfs = TempPdfs::new("missing");
        let path = pdfs.write("doc.pdf", &build_pdf(&["Alpha"]));
        let error = read_pdf_attachment_with(
            Path::new("/nonexistent/bin/pdftotext"),
            &path,
            PdfLimits::default(),
        )
        .expect_err("a missing executable must be reported");
        match error {
            PdfInputError::MissingTool(ref tool) => {
                assert_eq!(tool, "/nonexistent/bin/pdftotext");
                let message = error.to_string();
                assert!(message.contains("poppler"), "{message}");
                assert!(message.contains("brew install poppler"), "{message}");
            }
            other => panic!("expected MissingTool, got {other:?}"),
        }
    }

    // S3: the child is an argv list, never a shell string — the file name cannot
    // become a command, and the options are exactly what this module documents.
    #[cfg(unix)]
    #[test]
    fn the_child_is_invoked_with_an_argv_list() {
        let pdfs = TempPdfs::new("argv");
        let record = pdfs.dir.join("args.txt");
        let script = pdfs.script(
            "fake-pdftotext",
            &format!(
                "for arg in \"$@\"; do printf '%s\\n' \"$arg\"; done > '{}'\nprintf 'Texto\\n\\n\\f'\n",
                record.display()
            ),
        );
        let path = pdfs.write("informe.pdf", &build_pdf(&["Texto"]));
        let attachment =
            read_pdf_attachment_with(&script, &path, PdfLimits::default()).expect("extract");
        assert_eq!(attachment.pages, vec!["Texto".to_string()]);
        let recorded = std::fs::read_to_string(&record).expect("read the recorded argv");
        assert_eq!(
            recorded.lines().collect::<Vec<_>>(),
            vec![
                "-q",
                "-enc",
                "UTF-8",
                "-eol",
                "unix",
                path.to_str().expect("utf-8 path"),
                "-",
            ]
        );
    }

    // S3: a child that does not finish is killed at the deadline and the answer is
    // the timeout, not a hung app.
    #[cfg(unix)]
    #[test]
    fn a_slow_extraction_is_killed_at_the_deadline() {
        let pdfs = TempPdfs::new("slow");
        let script = pdfs.script("slow-pdftotext", "sleep 5");
        let path = pdfs.write("doc.pdf", &build_pdf(&["Alpha"]));
        let limits = PdfLimits {
            timeout: Duration::from_millis(200),
            ..PdfLimits::default()
        };
        let started = Instant::now();
        let error = read_pdf_attachment_with(&script, &path, limits)
            .expect_err("a slow extraction must time out");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "killed promptly"
        );
        assert_eq!(error, PdfInputError::TimedOut { seconds: 0 });
    }

    // S3: page and character ceilings refuse the document instead of truncating it
    // silently, and the refusal says which bound was crossed.
    #[test]
    fn the_page_and_text_bounds_refuse_the_document() {
        if !pdftotext_available() {
            eprintln!("skipping: pdftotext is not installed");
            return;
        }
        let pdfs = TempPdfs::new("bounds");
        let path = pdfs.write("doc.pdf", &build_pdf(&["Alpha", "Beta", "Gamma"]));
        let one_page = PdfLimits {
            pages: 1,
            ..PdfLimits::default()
        };
        assert_eq!(
            read_pdf_attachment_with(Path::new(PDFTOTEXT), &path, one_page),
            Err(PdfInputError::TooManyPages { pages: 3, limit: 1 })
        );
        let few_chars = PdfLimits {
            chars: 3,
            ..PdfLimits::default()
        };
        assert!(matches!(
            read_pdf_attachment_with(Path::new(PDFTOTEXT), &path, few_chars),
            Err(PdfInputError::TooMuchText { .. })
        ));
    }

    // The output ceiling is enforced while reading, so a child that floods stdout is
    // stopped by the byte cap rather than growing the app's memory.
    #[cfg(unix)]
    #[test]
    fn a_flood_of_output_is_cut_at_the_byte_cap() {
        let pdfs = TempPdfs::new("flood");
        let script = pdfs.script(
            "flood-pdftotext",
            "printf 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'\n",
        );
        let path = pdfs.write("doc.pdf", &build_pdf(&["Alpha"]));
        let limits = PdfLimits {
            chars: 4,
            ..PdfLimits::default()
        };
        assert!(matches!(
            read_pdf_attachment_with(&script, &path, limits),
            Err(PdfInputError::TooMuchText { .. })
        ));
    }

    // The splitter is the page-numbering rule itself: the form feed separates pages,
    // the one poppler writes after the last page is not a page, and trailing blanks
    // inside a page are whitespace rather than content.
    #[test]
    fn pages_are_split_on_the_form_feed() {
        assert_eq!(split_pages("Alpha\n\n\u{c}"), vec!["Alpha".to_string()]);
        assert_eq!(
            split_pages("Alpha\n\n\u{c}\u{c}Gamma\n\n\u{c}"),
            vec!["Alpha".to_string(), String::new(), "Gamma".to_string()]
        );
        assert_eq!(split_pages(""), vec![String::new()]);
        assert_eq!(split_pages("\u{c}"), vec![String::new()]);
        assert_eq!(
            split_pages("Alpha\u{c}Beta"),
            vec!["Alpha".to_string(), "Beta".to_string()]
        );
    }
}
