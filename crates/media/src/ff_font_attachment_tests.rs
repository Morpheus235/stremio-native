//! An embedded font is held ONCE by us, not once more by FFmpeg for the whole playback.
//!
//! `tests/fixtures/ass-font-attachment.mkv` (14 KB) is one ASS track and one TrueType attachment
//! (`pkg/fonts/subs/NotoSansThaana-Bold.ttf`, 13,176 bytes), made with
//! `ffmpeg -i fx.ass -map 0 -c:s ass -attach NotoSansThaana-Bold.ttf -metadata:s:t:0 mimetype=font/ttf
//! -metadata:s:t:0 filename=NotoSansThaana-Bold.ttf -fflags +bitexact`. The anime case this guards:
//! 25 MB of attached fonts were resident four times over during playback (FFmpeg's attachment
//! buffer and its codecpar copy, the subtitle store's copy and libass's), 100 MB of a 32-bit TV.
//! Needs the host FFmpeg, so it is `#[ignore]`d like the other `ff_*_tests`.

use super::image_subtitle_tests::bind_host_ffmpeg;
use super::*;
use crate::player::ass_source;

#[test]
#[ignore = "requires the bundled host FFmpeg; run make check-ffmpeg"]
fn an_attached_font_is_copied_out_and_ffmpegs_copy_released() {
    let _g = sn_base::testlock::serial();
    let _ffmpeg = bind_host_ffmpeg();
    ass_source::reset();
    unsafe {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/ass-font-attachment.mkv");
        let path = CString::new(path.to_str().unwrap()).unwrap();
        let mut fmt = avformat_alloc_context();
        assert!(avformat_open_input(&mut fmt, path.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut()) >= 0);
        let streams = (*fmt).streams;
        let attachment = (0..(*fmt).nb_streams as usize)
            .map(|i| stream_codecpar(*streams.add(i)))
            .find(|&cp| (*cp).codec_type == AVMEDIA_TYPE_ATTACHMENT)
            .expect("the fixture carries a font attachment");
        assert_eq!((*attachment).extradata_size, 13_176);
        let mut subs = SubTracks::open(fmt, streams);
        begin_ass_sources("fixture:font-attachment", fmt, streams, subs.entries());
        let (_, fonts) = ass_source::font_snapshot();
        assert_eq!(fonts.len(), 1, "the font reaches the subtitle store");
        assert_eq!(fonts[0].name, "NotoSansThaana-Bold.ttf");
        assert_eq!(fonts[0].data.len(), 13_176);
        assert!(
            (*attachment).extradata.is_null() && (*attachment).extradata_size == 0,
            "FFmpeg's copy is released once ours is taken"
        );
        subs.close();
        avformat_close_input(&mut fmt);
    }
    ass_source::reset();
}
