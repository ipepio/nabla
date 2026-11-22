//! Pure-Rust Markdown to Word (`.docx`) exporter.
//!
//! Converts a local Markdown file into a valid, editable OOXML `.docx` archive
//! without external executables, network access, or extra crates.
//!
//! Supported Markdown constructs:
//! - `#` through `######` headings (mapped to Word `Heading1`–`Heading6` styles)
//! - Paragraphs with inline `**bold**`, `*italic*`, `` `code` ``, and `[text](url)` links
//! - Ordered (`1.`) and unordered (`-`, `*`, `+`) lists using native `w:numPr`
//! - GFM pipe tables (`w:tbl`) with a bold repeatable header row
//!
//! Safety invariants:
//! - Never overwrites an existing destination file.
//! - Strips a single leading `@` from input/output paths (common in LLM tool calls).
//! - Requires the destination parent directory to already exist.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Strip a single leading `@` if present (LLM callers often pass `@report.md`).
fn normalize_path_arg(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed.strip_prefix('@').unwrap_or(trimmed)
}

/// Derive the default `.docx` output path next to `input`.
pub fn default_output_path(input: &Path) -> PathBuf {
    if input
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
    {
        input.with_extension("docx")
    } else {
        let mut s = input.as_os_str().to_os_string();
        s.push(".docx");
        PathBuf::from(s)
    }
}

/// Convert a Markdown file on disk into a new `.docx` file.
/// Returns the canonical/absolute path of the written `.docx` file.
pub fn export_markdown_to_docx(
    input_raw: &str,
    output_raw: Option<&str>,
) -> Result<PathBuf, String> {
    let input_str = normalize_path_arg(input_raw);
    if input_str.is_empty() {
        return Err("input Markdown path is required".to_string());
    }
    let input_path = PathBuf::from(input_str);
    if !input_path.is_file() {
        return Err(format!("input file does not exist: {}", input_path.display()));
    }

    let output_path = match output_raw.map(normalize_path_arg).filter(|s| !s.is_empty()) {
        Some(out) => PathBuf::from(out),
        None => default_output_path(&input_path),
    };

    if output_path.exists() {
        return Err(format!(
            "output file already exists (never overwritten): {}",
            output_path.display()
        ));
    }
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        return Err(format!(
            "destination directory does not exist: {}",
            parent.display()
        ));
    }

    let markdown = fs::read_to_string(&input_path)
        .map_err(|err| format!("failed to read {}: {err}", input_path.display()))?;
    let base_dir = input_path.parent();
    let bytes = markdown_to_docx_bytes_with_base(&markdown, base_dir);

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|err| format!("failed to create {}: {err}", output_path.display()))?;
    file.write_all(&bytes)
        .map_err(|err| format!("failed to write {}: {err}", output_path.display()))?;

    Ok(output_path.canonicalize().unwrap_or(output_path))
}

// ── Markdown parser & OOXML emitter ─────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
enum Block {
    Heading { level: u8, text: String },
    Paragraph(String),
    BulletItem(String),
    OrderedItem(String),
    Image { alt: String, src: String },
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
}

fn parse_blocks(markdown: &str) -> Vec<Block> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim_end();
        let trimmed = line.trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // Fenced code blocks degrade to plain paragraphs line by line.
        if trimmed.starts_with("```") {
            i += 1;
            let mut code_lines = Vec::new();
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                code_lines.push(lines[i]);
                i += 1;
            }
            if i < lines.len() {
                i += 1;
            }
            if !code_lines.is_empty() {
                blocks.push(Block::Paragraph(code_lines.join("\n")));
            }
            continue;
        }

        // ATX headings (# through ######).
        if let Some((level, text)) = parse_heading(trimmed) {
            blocks.push(Block::Heading {
                level,
                text: text.to_string(),
            });
            i += 1;
            continue;
        }

        // GFM table: header row + separator row (`| --- | --- |`).
        if trimmed.contains('|')
            && i + 1 < lines.len()
            && is_table_separator(lines[i + 1].trim())
        {
            let headers = split_table_row(trimmed);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() {
                let row_line = lines[i].trim();
                if row_line.is_empty() || !row_line.contains('|') {
                    break;
                }
                rows.push(split_table_row(row_line));
                i += 1;
            }
            blocks.push(Block::Table { headers, rows });
            continue;
        }

        // Standalone Markdown image block: ![alt](path)
        if let Some((alt, src)) = parse_image_block(trimmed) {
            blocks.push(Block::Image {
                alt: alt.to_string(),
                src: src.to_string(),
            });
            i += 1;
            continue;
        }

        // Unordered list item.
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            blocks.push(Block::BulletItem(rest.trim().to_string()));
            i += 1;
            continue;
        }

        // Ordered list item (`1. text`).
        if let Some(rest) = parse_ordered_item(trimmed) {
            blocks.push(Block::OrderedItem(rest.to_string()));
            i += 1;
            continue;
        }

        // Regular paragraph: collect consecutive plain lines.
        let mut para = vec![trimmed.strip_prefix("> ").unwrap_or(trimmed)];
        i += 1;
        while i < lines.len() {
            let next = lines[i].trim();
            if next.is_empty()
                || next.starts_with("```")
                || parse_heading(next).is_some()
                || next.starts_with("- ")
                || next.starts_with("* ")
                || next.starts_with("+ ")
                || parse_ordered_item(next).is_some()
                || (next.contains('|')
                    && i + 1 < lines.len()
                    && is_table_separator(lines[i + 1].trim()))
            {
                break;
            }
            para.push(next.strip_prefix("> ").unwrap_or(next));
            i += 1;
        }
        blocks.push(Block::Paragraph(para.join(" ")));
    }

    blocks
}

fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = line[hashes..].strip_prefix(' ')?;
    Some((hashes as u8, rest.trim()))
}

fn parse_image_block(line: &str) -> Option<(&str, &str)> {
    let after_bang = line.strip_prefix("![")?;
    let close_bracket = after_bang.find("](")?;
    let alt = &after_bang[..close_bracket];
    let rest = &after_bang[close_bracket + 2..];
    let src = rest.strip_suffix(')')?.trim();
    if src.is_empty() {
        return None;
    }
    Some((alt, src))
}

fn parse_ordered_item(line: &str) -> Option<&str> {
    let digits = line.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    line[digits..].strip_prefix(". ").map(str::trim)
}

fn is_table_separator(line: &str) -> bool {
    if !line.contains('-') {
        return false;
    }
    let cells = split_table_row(line);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let s = c.trim();
            !s.is_empty() && s.bytes().all(|b| b == b'-' || b == b':')
        })
}

fn split_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim().trim_matches('|');
    trimmed
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn emit_run(out: &mut String, text: &str, bold: bool, italic: bool) {
    if text.is_empty() {
        return;
    }
    out.push_str("<w:r>");
    if bold || italic {
        out.push_str("<w:rPr>");
        if bold {
            out.push_str("<w:b/>");
        }
        if italic {
            out.push_str("<w:i/>");
        }
        out.push_str("</w:rPr>");
    }
    let _ = write!(out, "<w:t xml:space=\"preserve\">{}</w:t></w:r>", xml_escape(text));
}

fn emit_inline(out: &mut String, text: &str, links: &mut Vec<(String, String)>, force_bold: bool) {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut buf = String::new();

    let flush = |buf: &mut String, out: &mut String| {
        if !buf.is_empty() {
            emit_run(out, buf, force_bold, false);
            buf.clear();
        }
    };

    while i < len {
        // Bold: **text**
        if i + 1 < len
            && chars[i] == '*'
            && chars[i + 1] == '*'
            && let Some(end) = find_double(&chars, i + 2, '*')
        {
            flush(&mut buf, out);
            let inner: String = chars[i + 2..end].iter().collect();
            emit_run(out, &inner, true, false);
            i = end + 2;
            continue;
        }
        // Italic: *text*
        if chars[i] == '*'
            && let Some(end) = find_single(&chars, i + 1, '*')
        {
            flush(&mut buf, out);
            let inner: String = chars[i + 1..end].iter().collect();
            emit_run(out, &inner, force_bold, true);
            i = end + 1;
            continue;
        }
        // Inline code: `text`
        if chars[i] == '`'
            && let Some(end) = find_single(&chars, i + 1, '`')
        {
            flush(&mut buf, out);
            let inner: String = chars[i + 1..end].iter().collect();
            emit_run(out, &inner, force_bold, false);
            i = end + 1;
            continue;
        }
        // Hyperlink: [label](url)
        if chars[i] == '['
            && let Some(close_bracket) = find_single(&chars, i + 1, ']')
            && close_bracket + 1 < len
            && chars[close_bracket + 1] == '('
            && let Some(close_paren) = find_single(&chars, close_bracket + 2, ')')
        {
            flush(&mut buf, out);
            let label: String = chars[i + 1..close_bracket].iter().collect();
            let url: String = chars[close_bracket + 2..close_paren].iter().collect();
            let rel_id = format!("rId{}", links.len() + 10);
            links.push((rel_id.clone(), url));
            let _ = write!(
                out,
                "<w:hyperlink r:id=\"{rel_id}\"><w:r><w:rPr><w:color w:val=\"0563C1\"/><w:u w:val=\"single\"/></w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r></w:hyperlink>",
                xml_escape(&label)
            );
            i = close_paren + 1;
            continue;
        }

        buf.push(chars[i]);
        i += 1;
    }

    flush(&mut buf, out);
}

fn find_single(chars: &[char], start: usize, needle: char) -> Option<usize> {
    (start..chars.len()).find(|&idx| chars[idx] == needle)
}

fn find_double(chars: &[char], start: usize, needle: char) -> Option<usize> {
    (start..chars.len().saturating_sub(1))
        .find(|&idx| chars[idx] == needle && chars[idx + 1] == needle)
}

struct EmbeddedImage {
    rel_id: String,
    zip_path: String,
    target_rel: String,
    bytes: Vec<u8>,
}

fn load_embedded_image(
    src: &str,
    base_dir: Option<&Path>,
    index: usize,
) -> Option<(EmbeddedImage, u64, u64)> {
    // Only local files are ever embedded (no network fetches).
    if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
        return None;
    }
    let raw_path = Path::new(src);
    let resolved = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else if let Some(base) = base_dir {
        base.join(raw_path)
    } else {
        raw_path.to_path_buf()
    };
    let bytes = fs::read(&resolved).ok()?;
    let (ext, width_px, height_px) = sniff_image_dimensions(&bytes)?;

    // Scale to fit a 6-inch content width (5,486,400 EMU) preserving aspect ratio.
    const EMU_PER_PX: u64 = 9525;
    const MAX_WIDTH_EMU: u64 = 5_486_400;
    let mut cx = u64::from(width_px.max(1)) * EMU_PER_PX;
    let mut cy = u64::from(height_px.max(1)) * EMU_PER_PX;
    if cx > MAX_WIDTH_EMU {
        cy = (cy * MAX_WIDTH_EMU) / cx;
        cx = MAX_WIDTH_EMU;
    }

    let rel_id = format!("rIdImg{index}");
    let target_rel = format!("media/image{index}.{ext}");
    let zip_path = format!("word/{target_rel}");
    Some((
        EmbeddedImage {
            rel_id,
            zip_path,
            target_rel,
            bytes,
        },
        cx,
        cy,
    ))
}

fn sniff_image_dimensions(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    // PNG: 8-byte signature + IHDR chunk (width at 16..20, height at 20..24).
    if bytes.len() >= 24 && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some(("png", w, h));
    }
    // JPEG: SOI (FF D8), walk markers until SOF0/SOF1/SOF2.
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let mut pos = 2;
        while pos + 8 < bytes.len() {
            if bytes[pos] != 0xFF {
                pos += 1;
                continue;
            }
            let marker = bytes[pos + 1];
            if matches!(marker, 0xC0..=0xC2) {
                let h = u16::from_be_bytes([bytes[pos + 5], bytes[pos + 6]]);
                let w = u16::from_be_bytes([bytes[pos + 7], bytes[pos + 8]]);
                return Some(("jpeg", u32::from(w), u32::from(h)));
            }
            if marker == 0xD9 || marker == 0xDA {
                break;
            }
            let seg_len = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
            if seg_len < 2 {
                break;
            }
            pos += 2 + seg_len;
        }
        return Some(("jpeg", 600, 400));
    }
    None
}

/// Convert Markdown text directly into `.docx` archive bytes.
#[cfg(test)]
pub fn markdown_to_docx_bytes(markdown: &str) -> Vec<u8> {
    markdown_to_docx_bytes_with_base(markdown, None)
}

/// Convert Markdown text into `.docx` bytes, resolving relative local images
/// against `base_dir` and embedding PNG/JPEG files natively in `word/media/`.
pub fn markdown_to_docx_bytes_with_base(markdown: &str, base_dir: Option<&Path>) -> Vec<u8> {
    let blocks = parse_blocks(markdown);
    let mut body = String::new();
    let mut links: Vec<(String, String)> = Vec::new();
    let mut images: Vec<EmbeddedImage> = Vec::new();

    for block in blocks {
        match block {
            Block::Heading { level, text } => {
                let _ = write!(
                    body,
                    "<w:p><w:pPr><w:pStyle w:val=\"Heading{level}\"/></w:pPr>"
                );
                emit_inline(&mut body, &text, &mut links, false);
                body.push_str("</w:p>");
            }
            Block::Paragraph(text) => {
                body.push_str("<w:p>");
                emit_inline(&mut body, &text, &mut links, false);
                body.push_str("</w:p>");
            }
            Block::BulletItem(text) => {
                body.push_str(
                    "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr>",
                );
                emit_inline(&mut body, &text, &mut links, false);
                body.push_str("</w:p>");
            }
            Block::OrderedItem(text) => {
                body.push_str(
                    "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"2\"/></w:numPr></w:pPr>",
                );
                emit_inline(&mut body, &text, &mut links, false);
                body.push_str("</w:p>");
            }
            Block::Image { alt, src } => {
                let img_idx = images.len() + 1;
                if let Some((embedded, cx, cy)) = load_embedded_image(&src, base_dir, img_idx) {
                    let rel_id = embedded.rel_id.clone();
                    let safe_alt = xml_escape(&alt);
                    let _ = write!(
                        body,
                        "<w:p><w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
                         <wp:extent cx=\"{cx}\" cy=\"{cy}\"/>\
                         <wp:docPr id=\"{img_idx}\" name=\"Image {img_idx}\" descr=\"{safe_alt}\"/>\
                         <a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\
                         <a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
                         <pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
                         <pic:nvPicPr><pic:cNvPr id=\"{img_idx}\" name=\"Image {img_idx}\"/><pic:cNvPicPr/></pic:nvPicPr>\
                         <pic:blipFill><a:blip r:embed=\"{rel_id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
                         <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
                         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr>\
                         </pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"
                    );
                    images.push(embedded);
                } else {
                    let label = if alt.trim().is_empty() {
                        format!("[image: {src}]")
                    } else {
                        format!("[image: {alt}]")
                    };
                    body.push_str("<w:p>");
                    emit_run(&mut body, &label, false, true);
                    body.push_str("</w:p>");
                }
            }
            Block::Table { headers, rows } => {
                body.push_str(
                    "<w:tbl><w:tblPr><w:tblBorders>\
                     <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BFBFBF\"/>\
                     <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BFBFBF\"/>\
                     <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BFBFBF\"/>\
                     <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BFBFBF\"/>\
                     <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D9D9D9\"/>\
                     <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D9D9D9\"/>\
                     </w:tblBorders></w:tblPr>",
                );
                body.push_str("<w:tr><w:trPr><w:tblHeader/></w:trPr>");
                for cell in &headers {
                    body.push_str("<w:tc><w:p>");
                    emit_inline(&mut body, cell, &mut links, true);
                    body.push_str("</w:p></w:tc>");
                }
                body.push_str("</w:tr>");
                for row in &rows {
                    body.push_str("<w:tr>");
                    for cell in row {
                        body.push_str("<w:tc><w:p>");
                        emit_inline(&mut body, cell, &mut links, false);
                        body.push_str("</w:p></w:tc>");
                    }
                    body.push_str("</w:tr>");
                }
                body.push_str("</w:tbl>");
            }
        }
    }

    let document_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
                     xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
                     xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\">\
         <w:body>{body}<w:sectPr/></w:body></w:document>"
    );

    let mut rels_xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
         <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>",
    );
    for (id, target) in &links {
        let _ = write!(
            rels_xml,
            "<Relationship Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>",
            xml_escape(target)
        );
    }
    for img in &images {
        let _ = write!(
            rels_xml,
            "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{}\"/>",
            img.rel_id, img.target_rel
        );
    }
    rels_xml.push_str("</Relationships>");

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
        <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
        <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
        <Default Extension=\"png\" ContentType=\"image/png\"/>\
        <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
        <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
        <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
        <Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>\
        </Types>";

    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
        </Relationships>";

    let styles_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
        <w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\"/><w:sz w:val=\"22\"/></w:rPr></w:rPrDefault></w:docDefaults>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:rPr><w:b/><w:sz w:val=\"36\"/></w:rPr></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:rPr><w:b/><w:sz w:val=\"30\"/></w:rPr></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/><w:rPr><w:b/><w:sz w:val=\"26\"/></w:rPr></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading4\"><w:name w:val=\"heading 4\"/><w:rPr><w:b/><w:sz w:val=\"24\"/></w:rPr></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading5\"><w:name w:val=\"heading 5\"/><w:rPr><w:b/><w:sz w:val=\"22\"/></w:rPr></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Heading6\"><w:name w:val=\"heading 6\"/><w:rPr><w:b/><w:sz w:val=\"20\"/></w:rPr></w:style>\
        </w:styles>";

    let numbering_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
        <w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
        <w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"&#x2022;\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl></w:abstractNum>\
        <w:abstractNum w:abstractNumId=\"1\"><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"%1.\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl></w:abstractNum>\
        <w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>\
        <w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num>\
        </w:numbering>";

    let mut zip_entries: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("word/document.xml", document_xml.as_bytes()),
        ("word/styles.xml", styles_xml.as_bytes()),
        ("word/numbering.xml", numbering_xml.as_bytes()),
        ("word/_rels/document.xml.rels", rels_xml.as_bytes()),
    ];
    for img in &images {
        zip_entries.push((&img.zip_path, &img.bytes));
    }
    build_stored_zip(&zip_entries)
}

// ── Minimal Stored ZIP archive builder (CRC32 + Method 0) ───────────────────

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn build_stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = Vec::new();
    let mut central = Vec::new();

    for &(name, data) in entries {
        let name_bytes = name.as_bytes();
        let offset = archive.len() as u32;
        let checksum = crc32(data);
        let size = data.len() as u32;

        // Local file header
        archive.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        archive.extend_from_slice(&20u16.to_le_bytes()); // version needed
        archive.extend_from_slice(&0u16.to_le_bytes()); // flags
        archive.extend_from_slice(&0u16.to_le_bytes()); // method 0 = stored
        archive.extend_from_slice(&0u16.to_le_bytes()); // mod time
        archive.extend_from_slice(&0u16.to_le_bytes()); // mod date
        archive.extend_from_slice(&checksum.to_le_bytes());
        archive.extend_from_slice(&size.to_le_bytes());
        archive.extend_from_slice(&size.to_le_bytes());
        archive.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes()); // extra len
        archive.extend_from_slice(name_bytes);
        archive.extend_from_slice(data);

        // Central directory header
        central.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // method 0
        central.extend_from_slice(&0u16.to_le_bytes()); // mod time
        central.extend_from_slice(&0u16.to_le_bytes()); // mod date
        central.extend_from_slice(&checksum.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra len
        central.extend_from_slice(&0u16.to_le_bytes()); // comment len
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }

    let central_offset = archive.len() as u32;
    let central_size = central.len() as u32;
    let count = entries.len() as u16;
    archive.extend_from_slice(&central);

    // End of central directory record
    archive.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    archive.extend_from_slice(&0u16.to_le_bytes()); // disk number
    archive.extend_from_slice(&0u16.to_le_bytes()); // start disk
    archive.extend_from_slice(&count.to_le_bytes());
    archive.extend_from_slice(&count.to_le_bytes());
    archive.extend_from_slice(&central_size.to_le_bytes());
    archive.extend_from_slice(&central_offset.to_le_bytes());
    archive.extend_from_slice(&0u16.to_le_bytes()); // comment len

    archive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_ooxml_zip_with_headings_lists_links_and_tables() {
        let md = "# Report\n\nHello **world** and *italic* with [link](https://example.com).\n\n- item a\n- item b\n\n1. first\n2. second\n\n| Col A | Col B |\n| --- | --- |\n| 1 | 2 |\n";
        let bytes = markdown_to_docx_bytes(md);
        assert!(bytes.starts_with(b"PK\x03\x04"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("word/document.xml"));
        assert!(text.contains("Heading1"));
        assert!(text.contains("<w:b/>"));
        assert!(text.contains("<w:i/>"));
        assert!(text.contains("https://example.com"));
        assert!(text.contains("<w:tbl>"));
    }

    #[test]
    fn refuses_to_overwrite_existing_output_file() {
        let dir = std::env::temp_dir().join(format!("nabla-docx-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let md_path = dir.join("note.md");
        let docx_path = dir.join("note.docx");
        fs::write(&md_path, "# Hi\n").unwrap();

        let first = export_markdown_to_docx(
            &format!("@{}", md_path.display()),
            Some(&docx_path.display().to_string()),
        );
        assert!(first.is_ok());

        let second = export_markdown_to_docx(
            &md_path.display().to_string(),
            Some(&docx_path.display().to_string()),
        );
        assert!(second.is_err());
        assert!(second.unwrap_err().contains("already exists"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn embeds_local_png_image_into_word_media_and_drawing() {
        let dir = std::env::temp_dir().join(format!("nabla-docx-img-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Minimal valid 1x1 PNG header + IHDR bytes.
        let fake_png: [u8; 24] = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A,
            0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R',
            0x00, 0x00, 0x00, 0x64, // 100px wide
            0x00, 0x00, 0x00, 0x32, // 50px high
        ];
        fs::write(dir.join("chart.png"), fake_png).unwrap();
        let bytes = markdown_to_docx_bytes_with_base("# Doc\n\n![Sales chart](chart.png)\n", Some(&dir));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("word/media/image1.png"));
        assert!(text.contains("<w:drawing>"));
        assert!(text.contains("r:embed=\"rIdImg1\""));
        let _ = fs::remove_dir_all(&dir);
    }
}
