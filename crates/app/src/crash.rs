//! What a crash leaves behind for the next reader: the running image's identity in the crash log
//! (written by `native/main.c` before the signal tracer is armed), and the Rust panic hook that
//! appends a panic's record to the same file.
//!
//! The signal handler (`native/crashtrace.c`) cannot parse ELF or allocate, so this normal-startup
//! marker is what pairs every later record with the binary that actually crashed, even if another
//! one is deployed before the log is read:
//!
//! ```text
//! img: build_id=<hex> image_addr=0x<lowest PT_LOAD vaddr> image_size=0x<PT_LOAD span>
//! ```
//!
//! or `img: unavailable` when the running executable's headers cannot be read (still a delimiter,
//! so a later crash never inherits the previous process's identity).

use std::os::raw::c_int;

use sn_base::eventlog::log;

/// The head of `/proc/self/exe` this reads: an ELF header and every program header after it, not
/// the whole binary.
const HEAD_BYTES: u64 = 64 * 1024;

/// What the marker says about one image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub build_id: String,
    pub image_addr: u64,
    pub image_size: u64,
}

/// The marker line for `image` (`None`: the identity could not be read).
pub fn marker_line(image: Option<&Image>) -> String {
    match image {
        Some(i) => format!("img: build_id={} image_addr=0x{:x} image_size=0x{:x}\n", i.build_id, i.image_addr, i.image_size),
        None => "img: unavailable\n".to_string(),
    }
}

/// Append the running image's identity to the crash log `fd`. Called by `native/main.c` while
/// allocation and file reads are still safe.
#[no_mangle]
pub extern "C" fn sn_crash_write_image_marker(fd: c_int) {
    if fd < 0 {
        return;
    }
    let image = read_head("/proc/self/exe").as_deref().and_then(parse_image);
    let line = marker_line(image.as_ref());
    // SAFETY: a write of a live local buffer to a descriptor the caller owns.
    unsafe {
        let _ = libc::write(fd, line.as_ptr().cast(), line.len());
    }
}

fn read_head(path: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(HEAD_BYTES).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// The identity of a little-endian `ET_EXEC` ELF (32- or 64-bit) from its head: the GNU build id
/// note, the lowest `PT_LOAD` vaddr and the span the `PT_LOAD`s cover. `None` for anything this
/// cannot answer honestly (not an ELF, big-endian, `ET_DYN` — whose vaddrs are offsets from a load
/// bias this cannot see — truncated headers, no build id).
pub fn parse_image(buf: &[u8]) -> Option<Image> {
    const ET_EXEC: u16 = 2;
    const PT_LOAD: u32 = 1;
    const PT_NOTE: u32 = 4;
    const NT_GNU_BUILD_ID: u32 = 3;
    if buf.len() < 64 || &buf[..4] != b"\x7fELF" || buf[5] != 1 {
        return None;
    }
    let is64 = match buf[4] {
        1 => false,
        2 => true,
        _ => return None,
    };
    let u16at = |o: usize| -> Option<u16> { Some(u16::from_le_bytes(buf.get(o..o + 2)?.try_into().ok()?)) };
    let u32at = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(buf.get(o..o + 4)?.try_into().ok()?)) };
    let u64at = |o: usize| -> Option<u64> { Some(u64::from_le_bytes(buf.get(o..o + 8)?.try_into().ok()?)) };
    let word = |o: usize| -> Option<u64> { if is64 { u64at(o) } else { u32at(o).map(u64::from) } };
    if u16at(16)? != ET_EXEC {
        return None;
    }
    let (phoff, phentsize, phnum) = if is64 {
        (u64at(32)? as usize, u16at(54)? as usize, u16at(56)? as usize)
    } else {
        (u32at(28)? as usize, u16at(42)? as usize, u16at(44)? as usize)
    };
    // Program header fields: ELF64 moves p_flags up to second, so the offsets differ by class.
    let (off_at, vaddr_at, filesz_at, memsz_at, min_entry) = if is64 { (8, 16, 32, 40, 56) } else { (4, 8, 16, 20, 32) };
    if phentsize < min_entry || phnum == 0 {
        return None;
    }
    let mut lo = u64::MAX;
    let mut hi = 0u64;
    let mut build_id = None;
    for i in 0..phnum {
        let base = phoff.checked_add(i.checked_mul(phentsize)?)?;
        match u32at(base)? {
            PT_LOAD => {
                let vaddr = word(base + vaddr_at)?;
                let memsz = word(base + memsz_at)?;
                lo = lo.min(vaddr);
                hi = hi.max(vaddr.checked_add(memsz)?);
            }
            PT_NOTE if build_id.is_none() => {
                let off = word(base + off_at)? as usize;
                let size = word(base + filesz_at)? as usize;
                let end = off.checked_add(size)?;
                let mut at = off;
                // `[namesz][descsz][type]`, then name and descriptor, each padded to four bytes.
                while at + 12 <= end && at + 12 <= buf.len() {
                    let namesz = u32at(at)? as usize;
                    let descsz = u32at(at + 4)? as usize;
                    let kind = u32at(at + 8)?;
                    let name_at = at + 12;
                    let desc_at = name_at.checked_add(namesz.next_multiple_of(4))?;
                    let desc_end = desc_at.checked_add(descsz)?;
                    if kind == NT_GNU_BUILD_ID && buf.get(name_at..name_at + namesz) == Some(b"GNU\0") {
                        let d = buf.get(desc_at..desc_end)?;
                        build_id = Some(d.iter().map(|b| format!("{b:02x}")).collect::<String>());
                        break;
                    }
                    let next = desc_at.checked_add(descsz.next_multiple_of(4))?;
                    if next <= at {
                        break;
                    }
                    at = next;
                }
            }
            _ => {}
        }
    }
    let build_id = build_id.filter(|b| !b.is_empty())?;
    (lo != u64::MAX).then(|| Image { build_id, image_addr: lo, image_size: hi - lo })
}

/// Route every Rust panic to the event log and append its record to the crash log, then run the
/// default hook (stderr, which `native/main.c` points at `stremionative-stderr.log`).
pub fn install_panic_logger() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_else(|| "?".into());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic>".into());
        let cur = std::thread::current();
        let line = format!("*** RUST PANIC [{}] at {loc}: {msg}", cur.name().unwrap_or("?"));
        log(&line);
        let _ = record_panic(
            &sn_base::paths::in_runtime_dir(sn_base::paths::runtime_file::CRASH),
            &line,
            unix_secs(),
        );
        default(info);
    }));
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Append a panic's record to the crash log at `path`: the line, scrubbed as every event-log line
/// is (a panic message is whatever the failing call was formatting, a URL included), then the
/// `time: <unix seconds>` line the C tracer also writes, in ONE write through the event log's
/// guarded open (`O_NOFOLLOW`, one link, this uid, 0640).
pub fn record_panic(path: &std::path::Path, line: &str, unix_secs: u64) -> std::io::Result<()> {
    let mut record = sn_base::eventlog::scrub::scrub_local(line);
    if unix_secs > 0 {
        record.push_str(&format!("\ntime: {unix_secs}"));
    }
    sn_base::eventlog::append_record(path, &record)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal 32-bit little-endian ET_EXEC: two PT_LOADs and a PT_NOTE holding a GNU build id.
    fn elf32(kind: u16, with_note: bool) -> Vec<u8> {
        let mut b = vec![0u8; 0x200];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 1; // ELFCLASS32
        b[5] = 1; // little-endian
        b[16..18].copy_from_slice(&kind.to_le_bytes());
        let phoff = 52u32;
        b[28..32].copy_from_slice(&phoff.to_le_bytes());
        b[42..44].copy_from_slice(&32u16.to_le_bytes());
        b[44..46].copy_from_slice(&3u16.to_le_bytes());
        let ph = |b: &mut Vec<u8>, i: usize, ty: u32, off: u32, vaddr: u32, filesz: u32, memsz: u32| {
            let at = phoff as usize + i * 32;
            b[at..at + 4].copy_from_slice(&ty.to_le_bytes());
            b[at + 4..at + 8].copy_from_slice(&off.to_le_bytes());
            b[at + 8..at + 12].copy_from_slice(&vaddr.to_le_bytes());
            b[at + 16..at + 20].copy_from_slice(&filesz.to_le_bytes());
            b[at + 20..at + 24].copy_from_slice(&memsz.to_le_bytes());
        };
        ph(&mut b, 0, 1, 0, 0x10000, 0x100, 0x5000);
        ph(&mut b, 1, 1, 0x100, 0x20000, 0x80, 0x3000);
        // a note before the build id, whose padding a careless parser would misread
        let note = 0x140usize;
        let mut n = Vec::new();
        n.extend_from_slice(&5u32.to_le_bytes()); // namesz "ABCD\0" (padded to 8)
        n.extend_from_slice(&4u32.to_le_bytes());
        n.extend_from_slice(&1u32.to_le_bytes());
        n.extend_from_slice(b"ABCD\0\0\0\0");
        n.extend_from_slice(&[9, 9, 9, 9]);
        if with_note {
            n.extend_from_slice(&4u32.to_le_bytes());
            n.extend_from_slice(&4u32.to_le_bytes());
            n.extend_from_slice(&3u32.to_le_bytes());
            n.extend_from_slice(b"GNU\0");
            n.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        }
        b[note..note + n.len()].copy_from_slice(&n);
        ph(&mut b, 2, 4, note as u32, 0, n.len() as u32, n.len() as u32);
        b
    }

    #[test]
    fn an_exec_image_reports_its_build_id_base_and_span() {
        let image = parse_image(&elf32(2, true)).expect("a readable ET_EXEC");
        assert_eq!(image, Image { build_id: "deadbeef".into(), image_addr: 0x10000, image_size: 0x13000 });
        assert_eq!(marker_line(Some(&image)), "img: build_id=deadbeef image_addr=0x10000 image_size=0x13000\n");
    }

    #[test]
    fn what_cannot_be_answered_honestly_is_unavailable() {
        assert_eq!(parse_image(&elf32(3, true)), None, "ET_DYN: vaddrs are offsets from an unseen bias");
        assert_eq!(parse_image(&elf32(2, false)), None, "no build id: no identity worth pairing on");
        assert_eq!(parse_image(b"not an elf at all, just some bytes padding it out to sixty-four"), None);
        let mut be = elf32(2, true);
        be[5] = 2;
        assert_eq!(parse_image(&be), None, "big-endian");
        assert_eq!(marker_line(None), "img: unavailable\n");
    }

    #[test]
    fn the_running_test_binary_has_an_identity_or_says_it_has_none() {
        // A host linker may or may not emit a build id; either way the marker is one line.
        let line = marker_line(read_head("/proc/self/exe").as_deref().and_then(parse_image).as_ref());
        assert!(line.starts_with("img: ") && line.ends_with('\n') && line.matches('\n').count() == 1, "{line}");
    }

    #[test]
    fn a_panic_record_is_scrubbed_and_carries_its_time() {
        let dir = std::env::temp_dir().join(format!("sn-panic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("crash.log");
        record_panic(&path, "*** RUST PANIC [art] at x.rs:1: bad url http://192.168.1.50:7000/manifest.json", 1_750_000_000).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("192.168.1.50"), "{written}");
        assert!(written.starts_with("*** RUST PANIC [art] at x.rs:1: "), "{written}");
        assert!(written.ends_with("\ntime: 1750000000\n"), "{written}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
