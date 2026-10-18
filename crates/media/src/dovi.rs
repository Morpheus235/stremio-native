//! **Dolby Vision Profile 7 → Profile 8.1, in the feed.** The C1's buffer feed takes ONE HEVC
//! elementary stream, so a dual-layer UHD Blu-ray stream (P7: base layer + enhancement layer +
//! RPU) is played as single-layer Profile 8.1 the way Plex, Infuse, Kodi and `dovi_tool convert
//! --mode 2` do: every enhancement-layer NAL unit (type 63, UNSPEC63) is dropped, and every RPU
//! (type 62, UNSPEC62) is rewritten from profile 7 to 8.1.
//!
//! The rewrite is a port of `dovi_tool`'s `ConversionMode::To81` (quietvoid/dovi_tool,
//! `dolby_vision` crate 3.4.0, MIT — see NOTICE and `licenses/dovi_tool-MIT.txt`):
//!
//! - header: `el_spatial_resampling_filter_flag = 0`, `disable_residual_flag = 1` (that pair is
//!   what makes `vdr_rpu_profile` 1 read as profile 8);
//! - mapping: the NLQ header (`nlq_method_idc`, `nlq_pred_pivot_value`) and the NLQ data are
//!   removed, `num_x/y_partitions_minus1` become 0; for an **FEL** RPU the luma/chroma curves
//!   are replaced by the identity polynomial (the residual that made them right is gone), for an
//!   **MEL** RPU they are kept;
//! - DM: the YCbCr→RGB and RGB→LMS matrices become the BT.2020 Profile 8.1 ones and
//!   `signal_color_space` 0; everything else — the L1/L2/L5/L6/L8/L9/L11/L254… extension
//!   blocks of CM v2.9 and v4.0 — is carried over **byte for byte**;
//! - the CRC32 (MPEG-2) is recomputed and the emulation-prevention bytes re-added.
//!
//! Unlike `dovi_tool` it does not decode the extension blocks: they start byte-aligned after
//! `dm_alignment_zero_bit` in both the source and the result, so they are copied as bytes. That
//! is equal to `dovi_tool`'s re-serialisation for every RPU it round-trips (which its writer
//! asserts through the CRC), and leaves alone the few it would refuse (an unknown block level).
//! One documented difference: data between the DM payload and the CRC (`remaining`) stays after
//! `rpu_alignment_zero_bit`, where the source put it; `dovi_tool` writes it before aligning.
//!
//! Runs on the demux thread for every frame: [`P81Feed`] owns its scratch buffers, so after the
//! first access unit nothing allocates.

/// The enhancement layer an RPU describes, from its NLQ data (`dovi_tool`'s `DoviELType`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ElType {
    /// Minimal EL: the NLQ parameters are all neutral, the EL adds nothing.
    Mel,
    /// Full EL: real residual data. Converting loses that enhancement detail, which is expected.
    Fel,
}

impl ElType {
    pub const fn as_str(self) -> &'static str {
        match self {
            ElType::Mel => "MEL",
            ElType::Fel => "FEL",
        }
    }
}

/// Why an RPU could not be converted (it is then dropped from the feed).
pub type Error = &'static str;

// -- CRC-32/MPEG-2 (poly 0x04C11DB7, init !0, not reflected, no final xor) ------------------

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 24;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

pub fn crc32_mpeg2(data: &[u8]) -> u32 {
    data.iter().fold(!0u32, |c, &b| (c << 8) ^ CRC_TABLE[((c >> 24) as u8 ^ b) as usize])
}

// -- emulation prevention --------------------------------------------------------------------

/// Remove every `00 00 03`'s `03`, judged on the ESCAPED bytes as `dovi_tool` does.
fn unescape(data: &[u8], out: &mut Vec<u8>) {
    out.clear();
    for (i, &b) in data.iter().enumerate() {
        if !(i >= 2 && b == 3 && data[i - 1] == 0 && data[i - 2] == 0) {
            out.push(b);
        }
    }
}

/// Append `data` escaped: a `03` before any byte <= 3 that follows two zeros.
fn escape_into(data: &[u8], out: &mut Vec<u8>) {
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
}

// -- bits --------------------------------------------------------------------------------------

struct Reader<'a> {
    d: &'a [u8],
    pos: usize, // in bits
}

impl Reader<'_> {
    fn bits(&mut self, n: u32) -> Result<u64, Error> {
        debug_assert!(n <= 64);
        if self.pos + n as usize > self.d.len() * 8 {
            return Err("RPU truncated");
        }
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | u64::from((self.d[self.pos >> 3] >> (7 - (self.pos & 7))) & 1);
            self.pos += 1;
        }
        Ok(v)
    }
    fn skip(&mut self, n: usize) -> Result<(), Error> {
        if self.pos + n > self.d.len() * 8 {
            return Err("RPU truncated");
        }
        self.pos += n;
        Ok(())
    }
    fn flag(&mut self) -> Result<bool, Error> {
        Ok(self.bits(1)? == 1)
    }
    fn ue(&mut self) -> Result<u64, Error> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros > 32 {
                return Err("RPU Exp-Golomb overflow");
            }
        }
        Ok((1u64 << zeros) - 1 + self.bits(zeros)?)
    }
    /// A signed Exp-Golomb value, only skipped (it is copied as bits).
    fn se(&mut self) -> Result<(), Error> {
        self.ue().map(drop)
    }
    /// Skip to the next byte boundary; the skipped bits must be zero (alignment bits).
    fn align(&mut self) -> Result<(), Error> {
        while self.pos & 7 != 0 {
            if self.flag()? {
                return Err("RPU alignment bit set");
            }
        }
        Ok(())
    }
}

struct Writer<'a> {
    out: &'a mut Vec<u8>,
    acc: u64,
    n: u32, // bits held in `acc`, < 8 between calls
}

impl Writer<'_> {
    fn put(&mut self, n: u32, v: u64) {
        let mut left = n;
        while left > 0 {
            let take = left.min(32);
            left -= take;
            let chunk = (v >> left) & ((1u64 << take) - 1);
            self.acc = (self.acc << take) | chunk;
            self.n += take;
            while self.n >= 8 {
                self.n -= 8;
                self.out.push((self.acc >> self.n) as u8);
            }
            self.acc &= (1u64 << self.n) - 1;
        }
    }
    fn ue(&mut self, v: u64) {
        let len = 64 - (v + 1).leading_zeros();
        self.put(len - 1, 0);
        self.put(len, v + 1);
    }
    fn se(&mut self, v: i64) {
        self.ue(if v > 0 { (v as u64) * 2 - 1 } else { v.unsigned_abs() * 2 });
    }
    fn align(&mut self) {
        if self.n > 0 {
            self.put(8 - self.n, 0);
        }
    }
    /// Copy the source bits `[from, to)` verbatim.
    fn copy(&mut self, src: &[u8], from: usize, to: usize) {
        let mut r = Reader { d: src, pos: from };
        let mut left = to - from;
        while left > 0 {
            let take = left.min(32) as u32;
            // in range by construction: `to` is a position the reader reached
            let v = r.bits(take).unwrap_or(0);
            self.put(take, v);
            left -= take as usize;
        }
    }
}

// -- the conversion ------------------------------------------------------------------------------

/// The profile-8.1 YCbCr→RGB (9 × i16, 3 × u32) and RGB→LMS (9 × i16) matrices,
/// `dovi_tool`'s `VdrDmData::set_p81_coeffs`.
const P81_YCC_TO_RGB: [i16; 9] = [9574, 0, 13802, 9574, -1540, -5348, 9574, 17610, 0];
const P81_YCC_OFFSETS: [u32; 3] = [16_777_216, 134_217_728, 134_217_728];
const P81_RGB_TO_LMS: [i16; 9] = [7222, 8771, 390, 2654, 12430, 1300, 0, 422, 15962];

/// The header fields the rest of the RPU's syntax depends on.
struct Header {
    format_ok: bool, // rpu_format & 0x700 == 0
    coefficient_data_type: u64,
    cdl: u32, // coefficient_log2_denom_length
    bl_bits: u32,
    el_bits: u32,
    disable_residual: bool,
    compressed_dm: bool,
    dm_present: bool,
    use_prev: bool,
}

/// Convert one HEVC UNSPEC62 NAL unit (`nal`: its 2-byte header and escaped payload, no start
/// code) to Profile 8.1, appending the converted NAL (header + escaped payload) to `out`.
/// `rbsp` and `rpu` are scratch. Returns the source RPU's enhancement-layer type (`None` for an
/// RPU that reuses a previous one's mapping, or one with no NLQ). On `Err`, `out` is unchanged.
pub fn rpu_to_p81(nal: &[u8], rbsp: &mut Vec<u8>, rpu: &mut Vec<u8>, out: &mut Vec<u8>) -> Result<Option<ElType>, Error> {
    if nal.len() < 3 || (nal[0] >> 1) & 0x3f != 62 {
        return Err("not an UNSPEC62 NAL unit");
    }
    unescape(&nal[2..], rbsp);
    let data = &rbsp[..];
    let trailing_zeroes = data.iter().rev().take_while(|&&b| b == 0).count();
    let end = data.len() - trailing_zeroes;
    if end < 7 || data[0] != 0x19 {
        return Err("not a Dolby Vision RPU");
    }
    if data[end - 1] != 0x80 {
        return Err("RPU final byte is not 0x80");
    }
    let crc_at = end - 5;
    let crc = u32::from_be_bytes([data[crc_at], data[crc_at + 1], data[crc_at + 2], data[crc_at + 3]]);
    if crc32_mpeg2(&data[1..crc_at]) != crc {
        return Err("RPU CRC32 mismatch");
    }
    rpu.clear();
    let body = &data[..crc_at];
    let el = convert_body(body, &mut Writer { out: rpu, acc: 0, n: 0 })?;
    let crc = crc32_mpeg2(&rpu[1..]);
    rpu.extend_from_slice(&crc.to_be_bytes());
    rpu.push(0x80);
    rpu.resize(rpu.len() + trailing_zeroes, 0);
    out.extend_from_slice(&nal[..2]);
    escape_into(rpu, out);
    Ok(el)
}

/// The RPU from its 0x19 prefix to just before the CRC32, written converted into `w` (which ends
/// byte-aligned).
fn convert_body(body: &[u8], w: &mut Writer) -> Result<Option<ElType>, Error> {
    let mut r = Reader { d: body, pos: 8 };
    w.put(8, 0x19);

    // rpu_data_header()
    let start = r.pos;
    if r.bits(6)? != 2 {
        return Err("rpu_type is not 2");
    }
    let rpu_format = r.bits(11)?;
    let vdr_rpu_profile = r.bits(4)?;
    let vdr_rpu_level = r.bits(4)?;
    let seq_info = r.flag()?;
    let mut h = Header {
        format_ok: rpu_format & 0x700 == 0,
        coefficient_data_type: 0,
        cdl: 0,
        bl_bits: 8,
        el_bits: 8,
        disable_residual: false,
        compressed_dm: false,
        dm_present: false,
        use_prev: false,
    };
    let (mut log2_denom, mut bl_m8, mut el_m8, mut vdr_m8, mut el_spatial) = (0, 0, 0, 0, false);
    if seq_info {
        r.bits(1)?; // chroma_resampling_explicit_filter_flag
        h.coefficient_data_type = r.bits(2)?;
        if h.coefficient_data_type == 0 {
            log2_denom = r.ue()?;
        }
        r.bits(3)?; // vdr_rpu_normalized_idc, bl_video_full_range_flag
        if h.format_ok {
            bl_m8 = r.ue()?;
            el_m8 = r.ue()? & 0xff; // the next 8 bits are ext_mapping_idc, kept as written
            vdr_m8 = r.ue()?;
            r.bits(1)?; // spatial_resampling_filter_flag
            h.compressed_dm = r.bits(3)? == 1; // reserved_zero_3bits
            el_spatial = r.flag()?;
            h.disable_residual = r.flag()?;
            // everything up to here is copied, then the two flags are rewritten for 8.1
            w.copy(body, start, r.pos - 2);
            w.put(1, 0); // el_spatial_resampling_filter_flag
            w.put(1, 1); // disable_residual_flag
        }
        h.cdl = match h.coefficient_data_type {
            0 => log2_denom as u32,
            1 => 32,
            _ => return Err("invalid coefficient_data_type"),
        };
    }
    if !(seq_info && h.format_ok) {
        // `dovi_tool` refuses these too (its bl_bit_depth_minus8 == 2 validation)
        return Err("RPU header without sequence info");
    }
    // `dovi_tool`'s profile: 1 + (el_spatial && !disable_residual) is 7 at a 12-bit VDR (4 else),
    // anything else with vdr_rpu_profile 1 is already 8.
    let profile = match vdr_rpu_profile {
        1 if el_spatial && !h.disable_residual => {
            if vdr_m8 == 4 {
                7
            } else {
                4
            }
        }
        1 => 8,
        _ => 0,
    };
    if profile != 7 && profile != 8 {
        return Err("RPU is not profile 7 or 8");
    }
    if vdr_rpu_level != 0 || bl_m8 != 2 || el_m8 != 2 || vdr_m8 > 6 || log2_denom > 23 {
        return Err("RPU header out of range");
    }
    h.bl_bits = bl_m8 as u32 + 8;
    h.el_bits = el_m8 as u32 + 8;
    let tail = r.pos;
    h.dm_present = r.flag()?;
    h.use_prev = r.flag()?;
    if h.use_prev {
        r.ue()?; // prev_vdr_rpu_id
    }
    w.copy(body, tail, r.pos);

    let el = if h.use_prev { None } else { convert_mapping(&mut r, w, body, &h, profile)? };

    if h.dm_present {
        convert_dm(&mut r, w, body, &h)?;
    }
    // rpu_alignment_zero_bit, then whatever precedes the CRC, as bytes
    r.align()?;
    w.align();
    w.out.extend_from_slice(&body[r.pos / 8..]);
    Ok(el)
}

/// `rpu_data_mapping()`, converted. Returns the EL type its NLQ data shows.
fn convert_mapping(r: &mut Reader, w: &mut Writer, body: &[u8], h: &Header, profile: u8) -> Result<Option<ElType>, Error> {
    let start = r.pos;
    r.ue()?; // vdr_rpu_id
    if r.ue()? != 0 || r.ue()? != 0 {
        return Err("mapping_color_space / mapping_chroma_format_idc not 0");
    }
    let ids_end = r.pos;
    let mut pieces = [0usize; 3];
    for p in &mut pieces {
        let minus2 = r.ue()?;
        if minus2 > 7 {
            return Err("too many pivots");
        }
        *p = minus2 as usize + 1;
        r.skip(h.bl_bits as usize * (minus2 as usize + 2))?; // pivots
    }
    let pivots_end = r.pos;
    let nlq = h.format_ok && !h.disable_residual;
    if nlq {
        if r.bits(3)? != 0 {
            return Err("nlq_method_idc is not linear-deadzone");
        }
        let a = r.bits(h.bl_bits)?;
        let b = r.bits(h.bl_bits)?;
        if profile == 7 && a + b != 1023 {
            return Err("nlq_pred_pivot_value does not sum to 1023");
        }
    }
    r.ue()?; // num_x_partitions_minus1
    r.ue()?; // num_y_partitions_minus1
    let params_start = r.pos;
    let cdt0 = h.coefficient_data_type == 0;
    for &n in &pieces {
        for _ in 0..n {
            match r.ue()? {
                0 => {
                    let order_minus1 = r.ue()?;
                    if order_minus1 > 1 {
                        return Err("poly_order_minus1 > 1");
                    }
                    if order_minus1 == 0 && r.flag()? {
                        return Err("linear interpolation mapping is unsupported");
                    }
                    for _ in 0..order_minus1 + 2 {
                        if cdt0 {
                            r.se()?;
                        }
                        r.bits(h.cdl)?;
                    }
                }
                1 => {
                    let order_minus1 = r.bits(2)?;
                    if order_minus1 > 2 {
                        return Err("mmr_order_minus1 > 2");
                    }
                    if cdt0 {
                        r.se()?;
                    }
                    r.bits(h.cdl)?;
                    for _ in 0..(order_minus1 + 1) * 7 {
                        if cdt0 {
                            r.se()?;
                        }
                        r.bits(h.cdl)?;
                    }
                }
                _ => return Err("invalid mapping_idc"),
            }
        }
    }
    let params_end = r.pos;
    let el = if nlq {
        // rpu_data_nlq(): MEL iff every parameter is neutral (`RpuDataNlq::is_mel`). With a
        // coefficient_data_type of 1 the `_int` parts are absent, which `dovi_tool` reads as 0 —
        // so `vdr_in_max_int` is never 1 and the RPU counts as FEL.
        let mut mel = true;
        for _ in 0..3 {
            mel &= r.bits(h.el_bits)? == 0; // nlq_offset
            mel &= cdt0 && r.ue()? == 1; // vdr_in_max_int
            mel &= r.bits(h.cdl)? == 0; // vdr_in_max
            for _ in 0..2 {
                // linear_deadzone_slope, linear_deadzone_threshold
                if cdt0 {
                    mel &= r.ue()? == 0;
                }
                mel &= r.bits(h.cdl)? == 0;
            }
        }
        Some(if mel { ElType::Mel } else { ElType::Fel })
    } else {
        None
    };

    // the converted mapping
    w.copy(body, start, ids_end);
    if el == Some(ElType::Fel) {
        // `RpuDataMapping::set_empty_p81_mapping`: per component one piece over [0, 1023], the
        // identity polynomial 0 + 1·x
        for _ in 0..3 {
            w.ue(0);
            w.put(h.bl_bits, 0);
            w.put(h.bl_bits, 1023);
        }
        w.ue(0);
        w.ue(0);
        for _ in 0..3 {
            w.ue(0); // mapping_idc: polynomial
            w.ue(0); // poly_order_minus1
            w.put(1, 0); // linear_interp_flag
            for coef_int in [0, 1] {
                if cdt0 {
                    w.se(coef_int);
                }
                w.put(h.cdl, 0);
            }
        }
    } else {
        w.copy(body, ids_end, pivots_end);
        w.ue(0);
        w.ue(0);
        w.copy(body, params_start, params_end);
    }
    Ok(el)
}

/// `vdr_dm_data_payload()` up to the CM v2.9 extension blocks, converted; leaves both sides
/// byte-aligned at the first extension block.
fn convert_dm(r: &mut Reader, w: &mut Writer, body: &[u8], h: &Header) -> Result<(), Error> {
    let start = r.pos;
    let affected = r.ue()?;
    r.ue()?; // current_dm_metadata_id
    r.ue()?; // scene_refresh_flag
    if affected > 15 {
        return Err("affected_dm_metadata_id > 15");
    }
    w.copy(body, start, r.pos);
    if !h.compressed_dm {
        r.skip(9 * 16 + 3 * 32 + 9 * 16)?;
        for c in P81_YCC_TO_RGB {
            w.put(16, u64::from(c as u16));
        }
        for o in P81_YCC_OFFSETS {
            w.put(32, u64::from(o));
        }
        for c in P81_RGB_TO_LMS {
            w.put(16, u64::from(c as u16));
        }
        let signal = r.pos;
        let eotf = r.bits(16)?;
        let params = r.bits(16)? | r.bits(16)? | r.bits(32)?;
        let depth = r.bits(5)?;
        if !(8..=16).contains(&depth) || (params == 0 && eotf != 65535) {
            return Err("DM signal description out of range");
        }
        w.copy(body, signal, r.pos);
        r.bits(2)?; // signal_color_space
        w.put(2, 0);
        let rest = r.pos;
        r.bits(2 + 2 + 12 + 12 + 10)?; // chroma format, full range, source min/max PQ, diagonal
        w.copy(body, rest, r.pos);
    }
    // CM v2.9: num_ext_blocks, dm_alignment_zero_bit
    let n = r.pos;
    r.ue()?;
    w.copy(body, n, r.pos);
    r.align()?;
    w.align();
    Ok(())
}

// -- the access unit -----------------------------------------------------------------------------

/// The per-playback Profile 7 rewriter of the demux thread: owns its scratch, counts, and logs
/// the conversion once (plus once more if a later RPU turns out to be FEL).
#[derive(Default)]
pub struct P81Feed {
    out: Vec<u8>,
    rbsp: Vec<u8>,
    rpu: Vec<u8>,
    units: Vec<(usize, usize, u8)>, // (start of the start code, start of the NAL header, type)
    /// RPUs converted, EL units dropped, RPUs dropped because they would not convert.
    pub converted: u64,
    pub el_dropped: u64,
    pub failed: u64,
    seen: Option<ElType>,
}

/// What one [`P81Feed::rewrite`] did, for the caller's log.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct AuReport {
    pub dropped: usize,
    pub converted: usize,
    /// The first time this playback saw an EL type (or saw FEL after MEL): log it.
    pub newly_seen: Option<ElType>,
    /// This AU's first conversion failure, if any.
    pub failure: Option<Error>,
}

impl P81Feed {
    /// Rewrite one Annex-B access unit in place: drop every NAL 63 and, when `convert`, rewrite
    /// every NAL 62 to Profile 8.1 (an RPU that will not convert is dropped); without `convert`,
    /// NAL 62 is dropped too (Profile 7 as its HDR10 base layer). Every other unit — start code
    /// included — is kept byte for byte. An AU with neither is not touched.
    pub fn rewrite(&mut self, au: &mut Vec<u8>, convert: bool) -> AuReport {
        let mut rep = AuReport::default();
        self.units.clear();
        let n = au.len();
        let mut i = 2;
        while i + 1 < n {
            // a start code is `00 00 01`; a byte > 1 cannot be any of its three
            let b = au[i];
            if b > 1 {
                i += 3;
            } else if b == 1 && au[i - 1] == 0 && au[i - 2] == 0 {
                let sc = if i >= 3 && au[i - 3] == 0 { i - 3 } else { i - 2 };
                self.units.push((sc, i + 1, (au[i + 1] >> 1) & 0x3f));
                i += 3;
            } else {
                i += 1;
            }
        }
        if !self.units.iter().any(|u| u.2 == 62 || u.2 == 63) {
            return rep;
        }
        self.out.clear();
        self.out.extend_from_slice(&au[..self.units[0].0]);
        for k in 0..self.units.len() {
            let (sc, hdr, t) = self.units[k];
            let end = self.units.get(k + 1).map_or(n, |u| u.0);
            match t {
                63 => rep.dropped += 1,
                62 if convert => {
                    let mark = self.out.len();
                    self.out.extend_from_slice(&au[sc..hdr]);
                    match rpu_to_p81(&au[hdr..end], &mut self.rbsp, &mut self.rpu, &mut self.out) {
                        Ok(el) => {
                            rep.converted += 1;
                            if el.is_some() && self.seen != Some(ElType::Fel) && el != self.seen {
                                self.seen = el;
                                rep.newly_seen = el;
                            }
                        }
                        Err(e) => {
                            self.out.truncate(mark);
                            rep.dropped += 1;
                            self.failed += 1;
                            rep.failure.get_or_insert(e);
                        }
                    }
                }
                62 => rep.dropped += 1,
                _ => self.out.extend_from_slice(&au[sc..end]),
            }
        }
        self.converted += rep.converted as u64;
        self.el_dropped += self.units.iter().filter(|u| u.2 == 63).count() as u64;
        std::mem::swap(au, &mut self.out);
        rep
    }
}

#[cfg(test)]
#[path = "dovi_tests.rs"]
mod tests;
