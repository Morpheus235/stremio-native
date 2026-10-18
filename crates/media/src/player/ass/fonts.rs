//! The faces every subtitle renderer starts with, and the order it adds them (plan D6).
//!
//! libass here runs with no font provider: it finds a face by FAMILY NAME among the faces handed
//! to `sn_ass_add_font`, and anything those faces lack falls to the default path (the CJK file,
//! with no coverage check). So Thai, Devanagari, Arabic and the other scripts of the account
//! language picker are reachable only because a face named for them is in the library:
//!
//!   * `appfont.ttf`, `appfont-bold.ttf` - Inter. Bold also answers to the family **"PlxSubs"**
//!     (a name record under Windows langID 0x0407), which the synthesized subtitle style names.
//!   * `fonts/subs/NotoSans<Script>-Bold.ttf` - one per script, cut by `tools/cut-noto-subs.py`,
//!     listed in `fonts/subs/manifest.json`. Each answers to "PlxSubs" and "Inter" as well as to its
//!     own name, and carries no Latin, digit or punctuation codepoint Inter Bold has.
//!   * `appfont-emoji.ttf` - the UI's monochrome emoji face (Noto Emoji, `tools/cut-noto-emoji.py`):
//!     pictographs only, with the same "PlxSubs"/"Inter" aliases, and win metrics that make libass
//!     draw its em at 0.8 of Inter Bold's (the UI's ratio), split at Inter Bold's ascent.
//!
//! **The order is load-bearing.** libass breaks a tie between equally good members of a family by
//! the order they were added, and takes the first member for code 0 without a coverage check, so
//! both Inter faces go in before every Noto face: Latin, digits and punctuation can never be drawn
//! by a Noto face. The Noto faces follow in manifest order, and the emoji face goes last.
//!
//! **Read at create.** The files are read from disk each time a renderer is created, handed to
//! libass (`ass_add_font` copies the bytes) and dropped. Nothing here is a process-lifetime copy:
//! the resident cost is libass's one copy per live renderer.

use super::Fault;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Inter Regular, then Inter Bold.
const INTER: [&str; 2] = ["appfont.ttf", "appfont-bold.ttf"];
/// The emoji face, shared with the UI chain, added after every other face.
const EMOJI: &str = "appfont-emoji.ttf";
/// Where the Noto faces and their manifest sit, under the app directory.
const SUBS_DIR: &str = "fonts/subs";
const MANIFEST: &str = "manifest.json";

#[derive(Deserialize)]
struct Manifest {
    faces: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    file: String,
}

/// Every bundled face, in the order the renderer adds it. A missing or unreadable manifest is
/// `FontsUnavailable`, like a missing Inter: a package without them is broken, and subtitles that
/// silently lose a script are worse than a subtitle error that says why.
pub(super) fn bundled(asset_dir: &Path) -> Result<Vec<PathBuf>, Fault> {
    let dir = asset_dir.join(SUBS_DIR);
    let manifest: Manifest = std::fs::read(dir.join(MANIFEST))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(Fault::FontsUnavailable)?;
    if manifest.faces.is_empty() {
        return Err(Fault::FontsUnavailable);
    }
    let mut files: Vec<PathBuf> = INTER.iter().map(|name| asset_dir.join(name)).collect();
    for entry in manifest.faces {
        // A bare file name: the manifest is payload, but a path out of the directory is never right.
        let plain = !entry.file.is_empty()
            && entry.file.ends_with(".ttf")
            && !entry.file.contains(['/', '\\', '\0'])
            && !entry.file.starts_with('.');
        if !plain {
            return Err(Fault::FontsUnavailable);
        }
        files.push(dir.join(entry.file));
    }
    files.push(asset_dir.join(EMOJI));
    Ok(files)
}

#[cfg(test)]
mod tests;
