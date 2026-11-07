//! What happens after the overlay closes: Lens search, OCR copy, notifications.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Opened in the browser, this page posts the image to Lens itself so the results session
/// belongs to the browser. no-referrer makes it send Origin: null, which Lens accepts
/// (a cross-site Origin gets 403).
/// shortcut: undocumented Lens endpoint, may change; if it breaks, check what lens.google.com's own upload posts.
const LENS_PAGE: &str = r#"<!doctype html><meta charset=utf-8><meta name=referrer content=no-referrer>
<title>Searching with Google Lens…</title>
<form id=f method=post enctype=multipart/form-data><input type=file name=encoded_image id=i></form>
<script>
const b = atob("@IMAGE@"), a = new Uint8Array(b.length);
for (let k = 0; k < b.length; k++) a[k] = b.charCodeAt(k);
const dt = new DataTransfer();
dt.items.add(new File([a], "s.png", {type: "image/png"}));
i.files = dt.files;
f.action = "https://lens.google.com/v3/upload?ep=ccm&s=&st=" + Date.now();
f.submit();
</script>"#;

pub fn png(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(rgb).expect("png data");
    writer.finish().expect("png finish");
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

/// Write the auto-uploading Lens page and open it in the default browser.
pub fn search(png: &[u8]) -> std::io::Result<()> {
    // Private runtime dir: fixed name so nothing piles up. Shared /tmp fallback: per-process name.
    let path = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("circle-search-lens.html"),
        None => std::env::temp_dir().join(format!("circle-search-lens-{}.html", std::process::id())),
    };
    let _ = std::fs::remove_file(&path);
    // create_new (O_EXCL) never follows a planted symlink; 0600 because the page holds the screenshot.
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    f.write_all(LENS_PAGE.replace("@IMAGE@", &base64(png)).as_bytes())?;
    Command::new("xdg-open").arg(&path).spawn()?;
    Ok(())
}

/// OCR the image with Tesseract and put the text on the clipboard; returns the text.
pub fn copy_text(png: &[u8]) -> Result<String, String> {
    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "OCR needs Tesseract: sudo apt install tesseract-ocr".to_string())?;
    child.stdin.take().unwrap().write_all(png).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("OCR failed: {}", err.trim().chars().take(200).collect::<String>()));
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !text.is_empty() {
        // xclip forks to keep serving the clipboard after we exit.
        let mut clip = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Copying needs xclip: sudo apt install xclip".to_string())?;
        clip.stdin.take().unwrap().write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        clip.wait().map_err(|e| e.to_string())?;
    }
    Ok(text)
}

pub fn notify(msg: &str, icon: &str) {
    let _ = Command::new("notify-send").args(["-i", icon, "Circle to Search", msg]).status();
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc4648() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(super::base64(i.as_bytes()), o);
        }
    }
}
