//! The Profile 7 → 8.1 RPU rewrite against `dovi_tool`'s own output.
//!
//! `tests/fixtures/dovi/<name>.bin` are RPUs from quietvoid/dovi_tool's `assets/tests` (MIT,
//! NOTICE), laid out as dovi_tool keeps them: a 4-byte start code, then the escaped RPU payload
//! without its 2-byte NAL header. `<name>.to81.bin` is what `dolby_vision` 3.4.0 (dovi_tool
//! 614c816) writes for it after `convert_with_mode(2)` — `dovi_tool convert --mode 2` — in the same
//! layout, generated with the library itself. `mel_to_81.bin` is dovi_tool's committed expectation
//! for `mel_orig.bin`. Its committed `fel_to_81.bin` is NOT the mode-2 output of `fel_orig.bin`:
//! dovi_tool's test makes it by converting to MEL first, so the mapping survives; mode 2 on an FEL
//! RPU sets the identity mapping, which `fel_orig.to81.bin` is.

use super::*;

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!("../../../tests/fixtures/dovi/", $name, ".bin")).as_slice()
    };
}

/// A fixture as the HEVC NAL unit the demuxer hands over: `7C 01` + payload.
fn nal(fixture: &[u8]) -> Vec<u8> {
    assert_eq!(&fixture[..4], &[0, 0, 0, 1]);
    [&[0x7C, 0x01][..], &fixture[4..]].concat()
}

fn convert(fixture: &[u8]) -> Result<(Vec<u8>, Option<ElType>), Error> {
    let (mut rbsp, mut rpu, mut out) = (Vec::new(), Vec::new(), Vec::new());
    let el = rpu_to_p81(&nal(fixture), &mut rbsp, &mut rpu, &mut out)?;
    Ok((out, el))
}

/// Every Profile 7 RPU fixture, its expected 8.1, and the EL type dovi_tool reports for it.
const CASES: &[(&str, &[u8], &[u8], Option<ElType>)] = &[
    ("fel_orig", fixture!("fel_orig"), fixture!("fel_orig.to81"), Some(ElType::Fel)),
    ("fel_rpu", fixture!("fel_rpu"), fixture!("fel_rpu.to81"), Some(ElType::Fel)),
    ("mel_orig", fixture!("mel_orig"), fixture!("mel_orig.to81"), Some(ElType::Mel)),
    ("mel_rpu", fixture!("mel_rpu"), fixture!("mel_rpu.to81"), Some(ElType::Mel)),
    ("fel_to_mel", fixture!("fel_to_mel"), fixture!("fel_to_mel.to81"), Some(ElType::Mel)),
    ("mel_to_mel", fixture!("mel_to_mel"), fixture!("mel_to_mel.to81"), Some(ElType::Mel)),
    ("data_before_crc32", fixture!("data_before_crc32"), fixture!("data_before_crc32.to81"), Some(ElType::Fel)),
    ("trailing_bytes_rpu", fixture!("trailing_bytes_rpu"), fixture!("trailing_bytes_rpu.to81"), Some(ElType::Fel)),
    ("mel_variable_l8_length13", fixture!("mel_variable_l8_length13"), fixture!("mel_variable_l8_length13.to81"), Some(ElType::Mel)),
    ("eof_rpu", fixture!("eof_rpu"), fixture!("eof_rpu.to81"), Some(ElType::Fel)),
    ("fix_se_write", fixture!("fix_se_write"), fixture!("fix_se_write.to81"), Some(ElType::Fel)),
    ("poly_coef_int_logic", fixture!("poly_coef_int_logic"), fixture!("poly_coef_int_logic.to81"), Some(ElType::Fel)),
    (
        "source_rpu_replaced_fel_orig",
        fixture!("source_rpu_replaced_fel_orig"),
        fixture!("source_rpu_replaced_fel_orig.to81"),
        Some(ElType::Fel),
    ),
];

#[test]
fn profile_7_rpus_convert_byte_for_byte_as_dovi_tool_mode_2() {
    for &(name, src, want, el) in CASES {
        let (got, got_el) = convert(src).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(got_el, el, "{name}: EL type");
        assert_eq!(&got[..2], &[0x7C, 0x01], "{name}: the NAL header is kept");
        assert_eq!(&got[2..], &want[4..], "{name}: payload differs from dovi_tool's");
    }
    // and against dovi_tool's own committed expectation
    let (got, _) = convert(fixture!("mel_orig")).unwrap();
    assert_eq!(&got[2..], &fixture!("mel_to_81")[4..]);
}

/// dovi_tool's To81 applies to a Profile 8 RPU too (new matrices, no NLQ to remove): the result
/// is what it writes, and converting that again changes nothing.
#[test]
fn a_profile_8_rpu_converts_as_dovi_tool_does_and_8_1_is_a_fixed_point() {
    let (got, el) = convert(fixture!("profile8")).unwrap();
    assert_eq!(el, None);
    assert_eq!(&got[2..], &fixture!("profile8.to81")[4..]);
    for &(name, _, want, _) in CASES {
        let (again, _) = convert(want).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&again[2..], &want[4..], "{name}: 8.1 → 8.1 is the identity");
    }
}

#[test]
fn crc32_is_mpeg2_and_a_corrupt_rpu_is_refused() {
    assert_eq!(crc32_mpeg2(b"123456789"), 0x0376_E6E7, "the CRC-32/MPEG-2 check value");
    // the converted RPU carries a CRC over its own bytes
    let (got, _) = convert(fixture!("fel_orig")).unwrap();
    let mut rbsp = Vec::new();
    unescape(&got[2..], &mut rbsp);
    let end = rbsp.len() - rbsp.iter().rev().take_while(|&&b| b == 0).count();
    assert_eq!(rbsp[end - 1], 0x80);
    let crc = u32::from_be_bytes(rbsp[end - 5..end - 1].try_into().unwrap());
    assert_eq!(crc32_mpeg2(&rbsp[1..end - 5]), crc);
    // one flipped bit in the source fails its CRC
    let mut bad = fixture!("mel_orig").to_vec();
    bad[40] ^= 0x10;
    assert_eq!(convert(&bad).err(), Some("RPU CRC32 mismatch"));
    assert!(convert(&[0, 0, 0, 1, 0x19, 0x08]).is_err(), "short");
}

#[test]
fn emulation_prevention_round_trips_and_escapes_every_start_code_lookalike() {
    let raw = [0x19, 0, 0, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 0, 0, 4, 0x80, 0, 0];
    let mut esc = Vec::new();
    escape_into(&raw, &mut esc);
    assert_eq!(esc, [0x19, 0, 0, 3, 0, 0, 3, 0, 1, 0, 0, 3, 2, 0, 0, 3, 3, 0, 0, 4, 0x80, 0, 0]);
    for w in esc.windows(3) {
        assert!(!(w[0] == 0 && w[1] == 0 && w[2] <= 2), "no start code or 00 00 00/02 in {esc:?}");
    }
    let mut back = Vec::new();
    unescape(&esc, &mut back);
    assert_eq!(back, raw);
    // the converted fixtures carry their escapes where dovi_tool put them (checked byte for byte
    // above); a source with escapes parses (fel_orig has several)
    assert!(fixture!("fel_orig").windows(3).any(|w| w == [0, 0, 3]));
}

// -- the feed: whole access units ----------------------------------------------------------

const AUD: &[u8] = &[0x46, 0x01, 0x50];
const IDR: &[u8] = &[0x26, 0x01, 0xAF, 0x00, 0x11];
const TRAIL: &[u8] = &[0x02, 0x01, 0xD0, 0x00];
const EL_VPS: &[u8] = &[0x7E, 0x01, 0x40, 0x01, 0x0C, 0x01];
const EL_SLICE: &[u8] = &[0x7E, 0x01, 0x26, 0x01, 0xAF, 0x09, 0x40];

fn annexb(units: &[&[u8]]) -> Vec<u8> {
    units.iter().flat_map(|u| [&[0u8, 0, 0, 1][..], u].concat()).collect()
}

#[test]
fn a_profile_7_access_unit_becomes_a_profile_8_1_one() {
    let mut feed = P81Feed::default();
    for &(name, src, want, el) in CASES {
        let rpu = nal(src);
        let mut au = annexb(&[AUD, IDR, EL_VPS, EL_SLICE, &rpu]);
        let rep = feed.rewrite(&mut au, true);
        assert_eq!((rep.dropped, rep.converted, rep.failure), (2, 1, None), "{name}");
        assert_eq!(au, annexb(&[AUD, IDR, &nal(want)]), "{name}: base layer kept, EL gone, RPU 8.1");
        let _ = el;
    }
    assert_eq!((feed.converted, feed.el_dropped, feed.failed), (CASES.len() as u64, 2 * CASES.len() as u64, 0));
}

#[test]
fn the_el_type_is_reported_once_and_again_only_when_fel_appears() {
    let mut feed = P81Feed::default();
    let mel = nal(fixture!("mel_orig"));
    let fel = nal(fixture!("fel_orig"));
    let mut seen = Vec::new();
    for rpu in [&mel, &mel, &fel, &mel, &fel] {
        let mut au = annexb(&[TRAIL, rpu]);
        seen.push(feed.rewrite(&mut au, true).newly_seen);
    }
    assert_eq!(seen, [Some(ElType::Mel), None, Some(ElType::Fel), None, None]);
}

#[test]
fn an_rpu_that_will_not_convert_is_dropped_and_counted() {
    let mut feed = P81Feed::default();
    let mut bad = nal(fixture!("mel_orig"));
    bad[30] ^= 0x01;
    let mut au = annexb(&[TRAIL, EL_SLICE, &bad]);
    let rep = feed.rewrite(&mut au, true);
    assert_eq!((rep.dropped, rep.converted, rep.failure), (2, 0, Some("RPU CRC32 mismatch")));
    assert_eq!(au, annexb(&[TRAIL]));
    assert_eq!(feed.failed, 1);
}

#[test]
fn non_dolby_vision_access_units_are_not_touched() {
    let mut feed = P81Feed::default();
    for au in [annexb(&[AUD, IDR]), annexb(&[TRAIL]), Vec::new(), vec![0, 0, 1], vec![0x7C, 0x01, 0x19]] {
        let mut x = au.clone();
        assert_eq!(feed.rewrite(&mut x, true), AuReport::default());
        assert_eq!(x, au);
    }
    // 3-byte start codes are kept as they are
    let au = [&[0u8, 0, 1][..], AUD, &[0, 0, 1], IDR, &[0, 0, 1], EL_SLICE].concat();
    let mut x = au.clone();
    assert_eq!(feed.rewrite(&mut x, true).dropped, 1);
    assert_eq!(x, [&[0u8, 0, 1][..], AUD, &[0, 0, 1], IDR].concat());
}

#[test]
fn without_convert_the_rpu_is_dropped_too() {
    let mut feed = P81Feed::default();
    let mut au = annexb(&[AUD, IDR, EL_SLICE, &nal(fixture!("fel_orig"))]);
    let rep = feed.rewrite(&mut au, false);
    assert_eq!((rep.dropped, rep.converted), (2, 0));
    assert_eq!(au, annexb(&[AUD, IDR]));
}
