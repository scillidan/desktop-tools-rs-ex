//! External OCR engines. RapidOCR is a persistent Python worker (the models
//! load once); Tesseract is a one-shot CLI with native word TSV. Both receive
//! the same upscaled capture as the Windows engine and return `Word` boxes in
//! capture pixels.

use super::Word;
use crate::logline;
use anyhow::{anyhow, bail, ensure, Context, Result};
use dt_common::layout::Rect;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x08000000;
const WORKER_PY: &str = include_str!("../../scripts/rapidocr_worker.py");
static RAPID: OnceLock<Mutex<Option<RapidWorker>>> = OnceLock::new();
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn rapid(
    w: i32,
    h: i32,
    bgra: &[u8],
    requested: Option<&str>,
) -> Result<Vec<Vec<Word>>> {
    let started = Instant::now();
    let image = TempPng::new(w, h, bgra)?;
    let lang = rapid_language(requested);
    let slot = RAPID.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(RapidWorker::spawn()?);
    }
    let result = guard.as_mut().unwrap().recognize(image.path(), lang);
    if result.is_err() {
        *guard = None; // a broken pipe or timeout must not poison the next OCR
    }
    let lines = result?;
    logline(&format!(
        "ocr: rapid {lang} -> {} lines, {} words in {} ms",
        lines.len(),
        lines.iter().map(Vec::len).sum::<usize>(),
        started.elapsed().as_millis()
    ));
    super::log_preview("ocr: rapid text", &lines);
    Ok(lines)
}

pub(super) fn tesseract(
    w: i32,
    h: i32,
    bgra: &[u8],
    requested: Option<&str>,
) -> Result<Vec<Vec<Word>>> {
    let started = Instant::now();
    let image = TempPng::new(w, h, bgra)?;
    let exe = executable("tesseract").context("tesseract.exe not found (install it with Scoop)")?;
    let lang = tesseract_language(requested);
    let output = Command::new(exe)
        .arg(image.path())
        .arg("stdout")
        .arg("-l")
        .arg(&lang)
        .arg("--psm")
        .arg("3")
        .arg("tsv")
        .env("OMP_THREAD_LIMIT", "1")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("cannot start tesseract")?;
    if !output.status.success() {
        bail!(
            "tesseract failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let tsv = String::from_utf8(output.stdout).context("tesseract returned non-UTF-8 TSV")?;
    let lines = parse_tsv(&tsv)?;
    logline(&format!(
        "ocr: tesseract {lang} -> {} lines, {} words in {} ms",
        lines.len(),
        lines.iter().map(Vec::len).sum::<usize>(),
        started.elapsed().as_millis()
    ));
    super::log_preview("ocr: tesseract text", &lines);
    Ok(lines)
}

struct TempPng(PathBuf);

impl TempPng {
    fn new(w: i32, h: i32, bgra: &[u8]) -> Result<Self> {
        ensure!(w > 0 && h > 0, "empty OCR capture");
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("dt-ocr-{}-{id}.png", std::process::id()));
        std::fs::File::create(&path).context("cannot create temporary OCR image")?;
        unsafe { crate::capture::write_png(&path, w as u32, h as u32, bgra)? };
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempPng {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct RapidWorker {
    child: Child,
    input: ChildStdin,
    output: Option<BufReader<ChildStdout>>,
}

impl RapidWorker {
    fn spawn() -> Result<Self> {
        let uv = executable("uv").context("uv.exe not found (install it with Scoop)")?;
        let script = std::env::temp_dir().join("dt-ocr-rapidocr-worker.py");
        if std::fs::read_to_string(&script).ok().as_deref() != Some(WORKER_PY) {
            std::fs::write(&script, WORKER_PY).context("cannot write RapidOCR worker")?;
        }
        let mut child = Command::new(uv)
            .arg("run")
            .arg(script)
            .arg("--worker")
            .env("UV_NO_PROGRESS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .context("cannot start RapidOCR worker")?;
        let input = child.stdin.take().context("RapidOCR stdin missing")?;
        let output = BufReader::new(child.stdout.take().context("RapidOCR stdout missing")?);
        Ok(Self {
            child,
            input,
            output: Some(output),
        })
    }

    fn recognize(&mut self, image: &Path, lang: &str) -> Result<Vec<Vec<Word>>> {
        writeln!(self.input, "{lang}\t{}", image.to_string_lossy())
            .and_then(|()| self.input.flush())
            .context("cannot send the RapidOCR request")?;
        let mut output = self.output.take().context("RapidOCR output is busy")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = read_rapid_response(&mut output);
            let _ = tx.send((result, output));
        });
        match rx.recv_timeout(Duration::from_secs(120)) {
            Ok((result, output)) => {
                self.output = Some(output);
                result
            }
            Err(e) => {
                let _ = self.child.kill();
                Err(anyhow!("RapidOCR worker did not answer: {e}"))
            }
        }
    }
}

impl Drop for RapidWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_rapid_response(output: &mut BufReader<ChildStdout>) -> Result<Vec<Vec<Word>>> {
    let mut line = String::new();
    output
        .read_line(&mut line)
        .context("RapidOCR closed stdout")?;
    if line.starts_with("ERROR\t") {
        bail!("RapidOCR: {}", line.trim_start_matches("ERROR\t").trim());
    }
    ensure!(
        line.trim() == "BEGIN",
        "invalid RapidOCR response: {:?}",
        line.trim()
    );
    let mut lines = Vec::new();
    loop {
        line.clear();
        if output.read_line(&mut line)? == 0 {
            bail!("RapidOCR response ended early");
        }
        if line.trim() == "END" {
            return Ok(lines);
        }
        parse_rapid_row(&line, &mut lines)?;
    }
}

fn parse_rapid_row(row: &str, lines: &mut Vec<Vec<Word>>) -> Result<()> {
    let mut fields = row.trim_end().splitn(3, '\t');
    let coords = fields.next().context("RapidOCR row has no box")?;
    let line: usize = fields.next().context("RapidOCR row has no line")?.parse()?;
    let text = fields.next().unwrap_or_default().trim().to_string();
    if text.is_empty() {
        return Ok(());
    }
    let xy: Vec<i32> = coords
        .split(',')
        .map(|v| v.parse().context("invalid RapidOCR coordinate"))
        .collect::<Result<_>>()?;
    ensure!(xy.len() == 4, "RapidOCR box must have four coordinates");
    while lines.len() <= line {
        lines.push(Vec::new());
    }
    lines[line].push(Word {
        text,
        line: line as u32,
        rect: Rect {
            l: xy[0],
            t: xy[1],
            r: xy[2],
            b: xy[3],
        },
    });
    Ok(())
}

fn parse_tsv(tsv: &str) -> Result<Vec<Vec<Word>>> {
    let mut out: Vec<Vec<Word>> = Vec::new();
    let mut current_key: Option<(String, String, String, String)> = None;
    for row in tsv.lines().skip(1) {
        let f: Vec<&str> = row.splitn(12, '\t').collect();
        if f.len() < 12 || f[0] != "5" || f[10].parse::<f32>().unwrap_or(-1.0) < 0.0 {
            continue;
        }
        let text = f[11].trim();
        if text.is_empty() {
            continue;
        }
        let left: i32 = f[6].parse().context("invalid tesseract left")?;
        let top: i32 = f[7].parse().context("invalid tesseract top")?;
        let width: i32 = f[8].parse().context("invalid tesseract width")?;
        let height: i32 = f[9].parse().context("invalid tesseract height")?;
        let key = (
            f[1].to_string(),
            f[2].to_string(),
            f[3].to_string(),
            f[4].to_string(),
        );
        if current_key.as_ref() != Some(&key) {
            out.push(Vec::new());
            current_key = Some(key);
        }
        let line = out.len() - 1;
        out[line].push(Word {
            text: text.to_string(),
            line: line as u32,
            rect: Rect {
                l: left,
                t: top,
                r: left + width,
                b: top + height,
            },
        });
    }
    Ok(out)
}

fn rapid_language(requested: Option<&str>) -> &str {
    let Some(requested) = requested else {
        return "ch";
    };
    let lang = requested.trim().to_ascii_lowercase();
    // The strings are only borrowed from `requested` for supported aliases;
    // normalized aliases below are static. Unknown values use the mixed model.
    match lang.as_str() {
        "en" | "en-us" | "en-gb" => "en",
        "zh" | "zh-cn" | "zh-hans" | "zh-hans-cn" => "ch",
        "zh-hant" | "zh-tw" | "zh-hk" => "chinese_cht",
        "ja" | "ja-jp" => "japan",
        "ko" | "ko-kr" => "korean",
        "ch" | "chinese_cht" | "japan" | "korean" | "latin" | "cyrillic" | "arabic"
        | "devanagari" | "ta" | "te" | "el" | "th" | "eslav" => match requested.trim() {
            "ch" | "chinese_cht" | "japan" | "korean" | "latin" | "cyrillic" | "arabic"
            | "devanagari" | "ta" | "te" | "el" | "th" | "eslav" => requested.trim(),
            _ => "ch",
        },
        other => {
            logline(&format!(
                "warn: RapidOCR language `{other}` unsupported; using ch"
            ));
            "ch"
        }
    }
}

fn tesseract_language(requested: Option<&str>) -> String {
    let Some(requested) = requested else {
        return "eng+chi_sim".to_string();
    };
    match requested.trim().to_ascii_lowercase().as_str() {
        "en" | "en-us" | "en-gb" => "eng".to_string(),
        "zh" | "zh-cn" | "zh-hans" | "zh-hans-cn" => "chi_sim".to_string(),
        "zh-hant" | "zh-tw" | "zh-hk" => "chi_tra".to_string(),
        _ => requested.trim().to_string(),
    }
}

fn executable(name: &str) -> Option<PathBuf> {
    let file = format!("{name}.exe");
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(&file);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let mut roots = Vec::new();
    if let Some(scoop) = std::env::var_os("SCOOP") {
        roots.push(PathBuf::from(scoop));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        roots.push(PathBuf::from(profile).join("scoop"));
    }
    roots.push(PathBuf::from(r"E:\Scoop"));
    roots
        .into_iter()
        .map(|root| root.join("shims").join(&file))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_rows_keep_native_word_boxes_and_line_numbers() {
        let mut lines = Vec::new();
        parse_rapid_row("8,43,105,76\t0\tPack\n", &mut lines).unwrap();
        parse_rapid_row("275,43,342,76\t0\t测\n", &mut lines).unwrap();
        parse_rapid_row("0,100,50,120\t1\tnext\n", &mut lines).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[0][0].text, "Pack");
        assert_eq!(
            lines[0][0].rect,
            Rect {
                l: 8,
                t: 43,
                r: 105,
                b: 76
            }
        );
        assert_eq!(lines[1][0].line, 1);
    }

    #[test]
    fn tesseract_tsv_groups_words_by_block_and_line() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   5\t1\t1\t1\t1\t1\t8\t43\t97\t33\t96.5\tPack\n\
                   5\t1\t1\t1\t1\t2\t116\t43\t38\t33\t96.0\tmy\n\
                   5\t1\t2\t1\t1\t1\t8\t100\t50\t20\t94.0\tnext\n";
        let lines = parse_tsv(tsv).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0].iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["Pack", "my"]
        );
        assert_eq!(
            lines[0][1].rect,
            Rect {
                l: 116,
                t: 43,
                r: 154,
                b: 76
            }
        );
        assert_eq!(lines[1][0].line, 1);
    }

    #[test]
    fn language_aliases_match_each_engine() {
        assert_eq!(rapid_language(None), "ch");
        assert_eq!(rapid_language(Some("zh-Hans-CN")), "ch");
        assert_eq!(rapid_language(Some("en-US")), "en");
        assert_eq!(tesseract_language(None), "eng+chi_sim");
        assert_eq!(tesseract_language(Some("zh-Hans-CN")), "chi_sim");
    }
}
