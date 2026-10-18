/* Software audio decode for codecs the television's audio path cannot take (FLAC, TrueHD, Opus,
 * Vorbis, MP3/MP2, ALAC, LPCM ...): decode with the bundled libavcodec and convert with
 * libswresample to S16 interleaved 48 kHz in exactly the layout the Load declared (stereo or
 * "6-channel" 5.1), so the declaration never depends on how accurate the metadata was.
 * swresample's downmix matrices fold 7.1 -> 5.1 and 5.1 -> stereo; a stereo source declared 5.1
 * is upmixed to FL/FR only. With SN_ADEC_DPLII or-ed into out_ch the stereo fold is swresample's
 * Dolby Pro Logic II matrix encoding (matrix_encoding=dplii; centre and surround at its default
 * -3 dB, LFE dropped), for a Pro Logic II receiver on a stereo link. DTS is decoded core only
 * (the DTS-HD extensions are lossless detail a stereo fold discards, at a CPU cost it does not).
 *
 * Ported from PlxNative's src/adec.c (plx_adec_* -> sn_adec_*; its media-probe half is not
 * needed here, crates/media probes in Rust). Built as libsnadec.so beside the bundled FFmpeg by
 * ci/build-adec.sh and compiled against ITS headers, so no FFmpeg struct offset is modelled by
 * hand on the Rust side (crates/media/src/adec.rs): Rust passes the AVCodecParameters and
 * AVPacket pointers it already holds and gets bytes back. */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavutil/channel_layout.h>
#include <libavutil/frame.h>
#include <libavutil/opt.h>
#include <libswresample/swresample.h>

/* 2: SN_ADEC_DPLII in sn_adec_open's out_ch. */
#define SN_ADEC_ABI 2
#define SN_ADEC_DPLII 0x100
#define OUT_RATE 48000

typedef struct sn_adec {
    AVCodecContext *ctx;
    AVFrame *frame;
    int out_ch;            /* 2 or 6, as declared */
    int dplii;             /* stereo as a Dolby Pro Logic II matrix */
    SwrContext *swr;       /* built for the input shape below; rebuilt whenever a frame differs */
    AVChannelLayout in_layout;
    int in_fmt, in_rate;
    uint8_t *buf;          /* S16LE interleaved */
    size_t len, cap;
    int64_t pts;           /* first frame's pts of the last decode, stream time base */
} sn_adec;

uint32_t sn_adec_abi_version(void) { return SN_ADEC_ABI; }

/* Why the last sn_adec_open returned NULL: 1 no decoder (+codec_id<<8), 2 alloc, 3 params,
 * 4 open2 (+(-AVERROR)<<8). */
static int last_err;
int sn_adec_last_error(void) { return last_err; }
unsigned sn_adec_lavc_version(void) { return avcodec_version(); }

/* Does the loaded libavcodec carry a decoder for FFmpeg codec NAME (`flac`, `truehd`, ...)? */
int sn_adec_has_decoder(const char *name) {
    const AVCodecDescriptor *d = avcodec_descriptor_get_by_name(name);
    return d && avcodec_find_decoder(d->id) != NULL;
}

sn_adec *sn_adec_open(const AVCodecParameters *par, int tb_num, int tb_den, int out_ch) {
    int dplii = (out_ch & SN_ADEC_DPLII) != 0;
    out_ch &= 0xff;
    const AVCodec *codec = avcodec_find_decoder(par->codec_id);
    last_err = 1 | (par->codec_id << 8);
    if (!codec) return NULL;
    last_err = 2;
    sn_adec *d = calloc(1, sizeof *d);
    if (!d) return NULL;
    d->ctx = avcodec_alloc_context3(codec);
    d->frame = av_frame_alloc();
    if (!d->ctx || !d->frame) goto fail;
    last_err = 3;
    if (avcodec_parameters_to_context(d->ctx, par) < 0) goto fail;
    d->ctx->pkt_timebase = (AVRational){tb_num, tb_den};
    /* TrueHD/MLP carry their own 2ch and 6ch presentations as substreams: ask for the one the Load
     * declared, so a 7.1 (Atmos) track decodes fewer channels with the mastering engineer's own
     * downmix instead of swresample's. Not every stream has one; the decoder then decodes all.
     * A Pro Logic II fold needs the surrounds: the 5.1 presentation. */
    if (par->codec_id == AV_CODEC_ID_TRUEHD || par->codec_id == AV_CODEC_ID_MLP)
        av_opt_set(d->ctx, "downmix", (out_ch == 6 || dplii) ? "5.1(side)" : "stereo", AV_OPT_SEARCH_CHILDREN);
    if (par->codec_id == AV_CODEC_ID_DTS)
        av_opt_set_int(d->ctx, "core_only", 1, AV_OPT_SEARCH_CHILDREN);
    int r = avcodec_open2(d->ctx, codec, NULL);
    if (r < 0) { last_err = 4 | (-r << 8); goto fail; }
    last_err = 0;
    d->out_ch = out_ch == 6 ? 6 : 2;
    d->dplii = dplii && d->out_ch == 2;
    return d;
fail:
    avcodec_free_context(&d->ctx);
    av_frame_free(&d->frame);
    free(d);
    return NULL;
}

int sn_adec_out_channels(const sn_adec *d) { return d->out_ch; }
int sn_adec_sample_rate(const sn_adec *d) { (void)d; return OUT_RATE; }

void sn_adec_flush(sn_adec *d) {
    avcodec_flush_buffers(d->ctx);
    swr_free(&d->swr); /* drop resampler history across a seek; rebuilt on the next frame */
    av_channel_layout_uninit(&d->in_layout);
}

void sn_adec_close(sn_adec *d) {
    if (!d) return;
    avcodec_free_context(&d->ctx);
    av_frame_free(&d->frame);
    swr_free(&d->swr);
    av_channel_layout_uninit(&d->in_layout);
    free(d->buf);
    free(d);
}

static int ensure_swr(sn_adec *d, const AVFrame *f) {
    /* A stream may change layout, format or rate mid-way (TrueHD, LPCM, AAC with SBR): swr reads
     * f->extended_data by the shape it was configured with, so a stale context reads past the
     * frame's planes. Rebuild on any change. */
    if (d->swr && f->format == d->in_fmt && f->sample_rate == d->in_rate &&
        !av_channel_layout_compare(&f->ch_layout, &d->in_layout))
        return 0;
    swr_free(&d->swr);
    av_channel_layout_uninit(&d->in_layout);
    if (f->ch_layout.nb_channels <= 0 || f->sample_rate <= 0) return -1;
    if (av_channel_layout_copy(&d->in_layout, &f->ch_layout) < 0) return -1;
    d->in_fmt = f->format;
    d->in_rate = f->sample_rate;
    AVChannelLayout in, out;
    if (av_channel_layout_copy(&in, &f->ch_layout) < 0) return -1;
    if (in.order == AV_CHANNEL_ORDER_UNSPEC) {
        int n = in.nb_channels;
        av_channel_layout_uninit(&in);
        av_channel_layout_default(&in, n);
    }
    if (d->out_ch == 6) out = (AVChannelLayout)AV_CHANNEL_LAYOUT_5POINT1;
    else out = (AVChannelLayout)AV_CHANNEL_LAYOUT_STEREO;
    int r = swr_alloc_set_opts2(&d->swr, &out, AV_SAMPLE_FMT_S16, OUT_RATE, &in, f->format, f->sample_rate, 0, NULL);
    av_channel_layout_uninit(&in);
    if (r < 0) return -1;
    if (d->dplii && av_opt_set(d->swr, "matrix_encoding", "dplii", 0) < 0) return -1;
    return swr_init(d->swr);
}

/* Convert one decoded frame onto the end of d->buf. */
static int append(sn_adec *d, const AVFrame *f) {
    if (ensure_swr(d, f) < 0) return -1;
    int max_out = swr_get_out_samples(d->swr, f->nb_samples);
    if (max_out < 0) return -1;
    size_t need = d->len + (size_t)max_out * d->out_ch * 2;
    if (need > d->cap) {
        uint8_t *b = realloc(d->buf, need * 2);
        if (!b) return -1;
        d->buf = b;
        d->cap = need * 2;
    }
    uint8_t *dst = d->buf + d->len;
    int n = swr_convert(d->swr, &dst, max_out, (const uint8_t **)f->extended_data, f->nb_samples);
    if (n < 0) return -1;
    d->len += (size_t)n * d->out_ch * 2;
    return 0;
}

/* Decode one demuxed packet. On return `out` and `len` hold every sample it produced as S16LE
 * interleaved at out_ch x 48 kHz, and `pts` the first frame's timestamp in the stream time base
 * (AV_NOPTS_VALUE when none). Returns 0, or a negative AVERROR. The buffer stays valid until the
 * next call. */
int sn_adec_decode(sn_adec *d, const AVPacket *pkt, const uint8_t **out, size_t *len, int64_t *pts) {
    d->len = 0;
    d->pts = AV_NOPTS_VALUE;
    int r = avcodec_send_packet(d->ctx, pkt);
    if (r < 0 && r != AVERROR(EAGAIN) && r != AVERROR_EOF) return r;
    while ((r = avcodec_receive_frame(d->ctx, d->frame)) >= 0) {
        if (d->pts == AV_NOPTS_VALUE) d->pts = d->frame->best_effort_timestamp;
        int a = append(d, d->frame);
        av_frame_unref(d->frame);
        if (a < 0) return AVERROR(ENOMEM);
    }
    *out = d->buf;
    *len = d->len;
    *pts = d->pts;
    return (r == AVERROR(EAGAIN) || r == AVERROR_EOF) ? 0 : r;
}
