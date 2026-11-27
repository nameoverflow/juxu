/*
 * csrc/voice_fft.c
 *
 * 1:1 Accelerate vDSP 11-band speech spectrum analyzer (`VoiceSpectrumAnalyzer` in `VoiceHUDPanel.swift`)
 * plus CoreGraphics / Objective-C runtime weak-proxy CADisplayLink & AppKit helpers for `juxu_macos_voice_ui`.
 */

#include <float.h>
#include <math.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#if defined(__APPLE__)
#include <Accelerate/Accelerate.h>
#include <ApplicationServices/ApplicationServices.h>
#include <CoreFoundation/CoreFoundation.h>
#include <CoreGraphics/CoreGraphics.h>
#include <objc/message.h>
#include <objc/objc.h>
#include <objc/runtime.h>

/* ==========================================================================
 * 1. Float32 Scratch Buffer Helpers (for Geisha tests & audio slices)
 * ========================================================================== */

void *juxu_voice_f32_buf_alloc(int32_t count)
{
    if (count <= 0) count = 1;
    return calloc((size_t)count, sizeof(float));
}

void juxu_voice_f32_buf_free(void *buf)
{
    if (buf != NULL) {
        free(buf);
    }
}

void juxu_voice_f32_buf_set(void *buf, int32_t idx, float val)
{
    if (buf != NULL && idx >= 0) {
        ((float *)buf)[idx] = val;
    }
}

float juxu_voice_f32_buf_get(const void *buf, int32_t idx)
{
    if (buf == NULL || idx < 0) return 0.0f;
    return ((const float *)buf)[idx];
}

/* ==========================================================================
 * 2. Accelerate vDSP 11-Band Speech Spectrum Analyzer (`VoiceSpectrumAnalyzer`)
 * ========================================================================== */

#define JUXU_VOICE_BAR_COUNT 11
#define JUXU_VOICE_FRAME_DURATION 0.025

static const float kBandEdgesHz[12] = {
    130.0f, 210.0f, 310.0f, 450.0f, 650.0f, 920.0f,
    1300.0f, 1800.0f, 2400.0f, 3100.0f, 3900.0f, 4800.0f
};

static const int32_t kBarToBand[11] = {
    9, 7, 0, 5, 2, 3, 4, 1, 6, 8, 10
};

static const float kBarTaper[11] = {
    0.66f, 0.78f, 0.88f, 0.94f, 0.98f, 1.00f, 0.98f, 0.94f, 0.88f, 0.78f, 0.66f
};

typedef struct JuxuVoiceSpectrum {
    double cached_sample_rate;
    int32_t nfft;
    vDSP_Length log2n;
    FFTSetup fft_setup;
    float *window;
    float window_norm;
    int32_t bin_lo[JUXU_VOICE_BAR_COUNT];
    int32_t bin_hi[JUXU_VOICE_BAR_COUNT];
    float tilt_db[JUXU_VOICE_BAR_COUNT];
    float floor_margins_db[JUXU_VOICE_BAR_COUNT];
    bool has_band_floor[JUXU_VOICE_BAR_COUNT];
    float band_floors_db[JUXU_VOICE_BAR_COUNT];
    float peak_above_db;
    float *frame_buf;
    float *real_part;
    float *imag_part;
    float *mag_sq;
} JuxuVoiceSpectrum;

void *juxu_voice_spectrum_new(void)
{
    JuxuVoiceSpectrum *s = (JuxuVoiceSpectrum *)calloc(1, sizeof(JuxuVoiceSpectrum));
    if (s == NULL) return NULL;
    s->window_norm = 1.0f;
    s->peak_above_db = 14.0f;
    for (int32_t b = 0; b < JUXU_VOICE_BAR_COUNT; b++) {
        float fc = sqrtf(kBandEdgesHz[b] * kBandEdgesHz[b + 1]);
        float fc_clamped = fc > 200.0f ? fc : 200.0f;
        s->tilt_db[b] = 2.8f * log2f(fc_clamped / 300.0f);
        s->floor_margins_db[b] = 4.8f - 0.8f * ((float)b / (float)(JUXU_VOICE_BAR_COUNT - 1));
        s->has_band_floor[b] = false;
        s->band_floors_db[b] = 0.0f;
    }
    return s;
}

void juxu_voice_spectrum_free(void *ptr)
{
    if (ptr == NULL) return;
    JuxuVoiceSpectrum *s = (JuxuVoiceSpectrum *)ptr;
    if (s->fft_setup != NULL) {
        vDSP_destroy_fftsetup(s->fft_setup);
        s->fft_setup = NULL;
    }
    free(s->window);
    free(s->frame_buf);
    free(s->real_part);
    free(s->imag_part);
    free(s->mag_sq);
    free(s);
}

static void juxu_voice_spectrum_ensure_configured(JuxuVoiceSpectrum *s, double sample_rate)
{
    if (s == NULL || sample_rate <= 0.0 || sample_rate == s->cached_sample_rate) {
        return;
    }
    if (s->fft_setup != NULL) {
        vDSP_destroy_fftsetup(s->fft_setup);
        s->fft_setup = NULL;
    }
    free(s->window);
    free(s->frame_buf);
    free(s->real_part);
    free(s->imag_part);
    free(s->mag_sq);

    s->cached_sample_rate = sample_rate;
    s->nfft = (sample_rate <= 24000.0) ? 512 : 1024;
    s->log2n = (vDSP_Length)llround(log2((double)s->nfft));
    s->fft_setup = vDSP_create_fftsetup(s->log2n, kFFTRadix2);

    int32_t half = s->nfft / 2;
    s->window = (float *)calloc((size_t)s->nfft, sizeof(float));
    s->frame_buf = (float *)calloc((size_t)s->nfft, sizeof(float));
    s->real_part = (float *)calloc((size_t)half, sizeof(float));
    s->imag_part = (float *)calloc((size_t)half, sizeof(float));
    s->mag_sq = (float *)calloc((size_t)half, sizeof(float));

    vDSP_hann_window(s->window, (vDSP_Length)s->nfft, vDSP_HANN_NORM);
    float norm = 0.0f;
    vDSP_dotpr(s->window, 1, s->window, 1, &norm, (vDSP_Length)s->nfft);
    s->window_norm = norm > 1e-6f ? norm : 1e-6f;

    float bin_hz = (float)sample_rate / (float)s->nfft;
    for (int32_t b = 0; b < JUXU_VOICE_BAR_COUNT; b++) {
        int32_t lo = (int32_t)floorf(kBandEdgesHz[b] / bin_hz);
        if (lo < 1) lo = 1;
        int32_t hi = (int32_t)ceilf(kBandEdgesHz[b + 1] / bin_hz);
        if (hi < lo + 1) hi = lo + 1;
        if (hi > half) hi = half;
        s->bin_lo[b] = lo;
        s->bin_hi[b] = hi;
    }
}

static void juxu_voice_spectrum_analyze_slice(
    JuxuVoiceSpectrum *s,
    const float *samples,
    int32_t count,
    float seconds,
    float *out_bars
)
{
    if (s == NULL || s->fft_setup == NULL || s->nfft <= 0 || samples == NULL || count <= 0) {
        for (int32_t i = 0; i < JUXU_VOICE_BAR_COUNT; i++) out_bars[i] = 0.0f;
        return;
    }

    int32_t nfft = s->nfft;
    int32_t copy_count = count < nfft ? count : nfft;
    int32_t pad = nfft - copy_count;
    if (pad > 0) {
        memset(s->frame_buf, 0, (size_t)pad * sizeof(float));
    }
    memcpy(s->frame_buf + pad, samples, (size_t)copy_count * sizeof(float));
    vDSP_vmul(s->frame_buf, 1, s->window, 1, s->frame_buf, 1, (vDSP_Length)nfft);

    int32_t half = nfft / 2;
    DSPSplitComplex split;
    split.realp = s->real_part;
    split.imagp = s->imag_part;
    vDSP_ctoz((const DSPComplex *)s->frame_buf, 2, &split, 1, (vDSP_Length)half);
    vDSP_fft_zrip(s->fft_setup, &split, 1, s->log2n, kFFTDirection_Forward);
    vDSP_zvmags(&split, 1, s->mag_sq, 1, (vDSP_Length)half);

    float power_scale = 1.0f / (2.0f * (float)nfft * s->window_norm);
    float raw_above[JUXU_VOICE_BAR_COUNT];
    float frame_max = -100.0f;

    for (int32_t b = 0; b < JUXU_VOICE_BAR_COUNT; b++) {
        float band_power = 0.0f;
        for (int32_t bin = s->bin_lo[b]; bin < s->bin_hi[b]; bin++) {
            band_power += s->mag_sq[bin];
        }
        band_power *= power_scale;
        float safe_power = band_power > 1e-12f ? band_power : 1e-12f;
        float db = 10.0f * log10f(safe_power) + s->tilt_db[b];
        float clamped = db < -85.0f ? -85.0f : (db > -38.0f ? -38.0f : db);
        if (s->has_band_floor[b]) {
            float fl = s->band_floors_db[b];
            float tau = (clamped < fl) ? 0.20f : 6.0f;
            s->band_floors_db[b] = fl + (clamped - fl) * (1.0f - expf(-seconds / tau));
        } else {
            s->has_band_floor[b] = true;
            s->band_floors_db[b] = clamped;
        }
        float floor_val = s->band_floors_db[b];
        float above = db - (floor_val + s->floor_margins_db[b]);
        raw_above[b] = above;
        if (above > frame_max) frame_max = above;
    }

    float target_peak = frame_max < 13.0f ? 13.0f : (frame_max > 36.0f ? 36.0f : frame_max);
    float peak_tau = (target_peak > s->peak_above_db) ? 0.25f : 4.0f;
    s->peak_above_db += (target_peak - s->peak_above_db) * (1.0f - expf(-seconds / peak_tau));
    float span = s->peak_above_db > 13.0f ? s->peak_above_db : 13.0f;

    for (int32_t i = 0; i < JUXU_VOICE_BAR_COUNT; i++) {
        float ratio = raw_above[kBarToBand[i]] / span;
        float norm = ratio < 0.0f ? 0.0f : (ratio > 1.0f ? 1.0f : ratio);
        out_bars[i] = powf(norm, 1.15f) * kBarTaper[i];
    }
}

int32_t juxu_voice_spectrum_frame_cap(int32_t count, double sample_rate)
{
    if (count <= 0 || sample_rate <= 0.0) return 0;
    int32_t step = (int32_t)llround(sample_rate * JUXU_VOICE_FRAME_DURATION);
    if (step < 1) step = 1;
    int32_t cap = (count + step - 1) / step;
    return cap > 1 ? cap : 1;
}

int32_t juxu_voice_spectrum_process(
    void *ptr,
    const float *samples,
    int32_t count,
    double sample_rate,
    float *out_bars,
    int32_t max_frames
)
{
    if (ptr == NULL || samples == NULL || out_bars == NULL || count <= 0 || sample_rate <= 0.0 || max_frames <= 0) {
        return 0;
    }
    JuxuVoiceSpectrum *s = (JuxuVoiceSpectrum *)ptr;
    juxu_voice_spectrum_ensure_configured(s, sample_rate);

    int32_t step = (int32_t)llround(sample_rate * JUXU_VOICE_FRAME_DURATION);
    if (step < 1) step = 1;

    int32_t frame_count = 0;
    int32_t offset = 0;
    while (offset < count && frame_count < max_frames) {
        int32_t end = offset + step;
        if (end > count) end = count;
        int32_t chunk = end - offset;
        if (chunk >= step / 2 || frame_count == 0) {
            int32_t slice_start = end - s->nfft;
            if (slice_start < 0) slice_start = 0;
            int32_t slice_count = end - slice_start;
            float sec = (float)((double)chunk / sample_rate);
            juxu_voice_spectrum_analyze_slice(
                s,
                samples + slice_start,
                slice_count,
                sec,
                out_bars + frame_count * JUXU_VOICE_BAR_COUNT
            );
            frame_count++;
        }
        offset = end;
    }
    return frame_count;
}

void juxu_voice_spectrum_process_frame(
    void *ptr,
    const float *samples,
    int32_t count,
    double sample_rate,
    bool has_seconds,
    float seconds,
    float *out_bars11
)
{
    if (out_bars11 == NULL) return;
    if (ptr == NULL || samples == NULL || count <= 0 || sample_rate <= 0.0) {
        for (int32_t i = 0; i < JUXU_VOICE_BAR_COUNT; i++) out_bars11[i] = 0.0f;
        return;
    }
    JuxuVoiceSpectrum *s = (JuxuVoiceSpectrum *)ptr;
    juxu_voice_spectrum_ensure_configured(s, sample_rate);
    int32_t slice_count = count < s->nfft ? count : s->nfft;
    float sec = has_seconds ? seconds : (float)((double)count / sample_rate);
    juxu_voice_spectrum_analyze_slice(
        s,
        samples + (count - slice_count),
        slice_count,
        sec,
        out_bars11
    );
}

/* ==========================================================================
 * 3. AppKit / QuartzCore / CoreGraphics Helpers & Weak DisplayLink Proxy
 * ========================================================================== */

int64_t juxu_voice_ui_floating_window_level(void)
{
    return (int64_t)CGWindowLevelForKey(kCGFloatingWindowLevelKey) + 2LL;
}

bool juxu_voice_ui_reduce_motion(void)
{
    Class wsCls = objc_getClass("NSWorkspace");
    if (wsCls == Nil) return false;
    typedef id (*shared_fn)(id, SEL);
    typedef bool (*bool_fn)(id, SEL);
    id ws = ((shared_fn)objc_msgSend)((id)wsCls, sel_registerName("sharedWorkspace"));
    if (ws == nil) return false;
    SEL sel = sel_registerName("accessibilityDisplayShouldReduceMotion");
    if (!((bool (*)(id, SEL, SEL))objc_msgSend)(ws, sel_registerName("respondsToSelector:"), sel)) {
        return false;
    }
    return ((bool_fn)objc_msgSend)(ws, sel);
}

void juxu_voice_ui_layer_continuous_corner(void *layer_ptr, double radius, bool masks_to_bounds)
{
    if (layer_ptr == NULL) return;
    id layer = (id)layer_ptr;
    typedef void (*set_f_fn)(id, SEL, double);
    typedef void (*set_b_fn)(id, SEL, bool);
    typedef void (*set_id_fn)(id, SEL, id);
    typedef id (*str_fn)(id, SEL, const char *);

    ((set_f_fn)objc_msgSend)(layer, sel_registerName("setCornerRadius:"), radius);
    Class nsStrCls = objc_getClass("NSString");
    if (nsStrCls != Nil) {
        id cont = ((str_fn)objc_msgSend)((id)nsStrCls, sel_registerName("stringWithUTF8String:"), "continuous");
        if (cont != nil && ((bool (*)(id, SEL, SEL))objc_msgSend)(layer, sel_registerName("respondsToSelector:"), sel_registerName("setCornerCurve:"))) {
            ((set_id_fn)objc_msgSend)(layer, sel_registerName("setCornerCurve:"), cont);
        }
    }
    ((set_b_fn)objc_msgSend)(layer, sel_registerName("setMasksToBounds:"), masks_to_bounds);
}

void juxu_voice_ui_layer_border_rgba(void *layer_ptr, double width, double r, double g, double b, double a)
{
    if (layer_ptr == NULL) return;
    id layer = (id)layer_ptr;
    typedef void (*set_f_fn)(id, SEL, double);
    typedef void (*set_cg_fn)(id, SEL, CGColorRef);

    ((set_f_fn)objc_msgSend)(layer, sel_registerName("setBorderWidth:"), width);
    if (width > 0.0) {
        CGColorRef col = CGColorCreateSRGB(r, g, b, a);
        if (col != NULL) {
            ((set_cg_fn)objc_msgSend)(layer, sel_registerName("setBorderColor:"), col);
            CGColorRelease(col);
        }
    } else {
        ((set_cg_fn)objc_msgSend)(layer, sel_registerName("setBorderColor:"), NULL);
    }
}

void juxu_voice_ui_bounding_rect(void *ns_str_ptr, double max_width, void *attrs_ptr, double *out_rect4)
{
    if (out_rect4 == NULL) return;
    out_rect4[0] = 0.0;
    out_rect4[1] = 0.0;
    out_rect4[2] = 0.0;
    out_rect4[3] = 0.0;
    if (ns_str_ptr == NULL) return;

    CGSize sz = { max_width, 1.0e7 };
    /* NSStringDrawingUsesLineFragmentOrigin == 1 */
    typedef CGRect (*bound_fn)(id, SEL, CGSize, long, id, id);
    SEL sel = sel_registerName("boundingRectWithSize:options:attributes:context:");
    if (((bool (*)(id, SEL, SEL))objc_msgSend)((id)ns_str_ptr, sel_registerName("respondsToSelector:"), sel)) {
        CGRect r = ((bound_fn)objc_msgSend)((id)ns_str_ptr, sel, sz, 1L, (id)attrs_ptr, nil);
        out_rect4[0] = r.origin.x;
        out_rect4[1] = r.origin.y;
        out_rect4[2] = r.size.width;
        out_rect4[3] = r.size.height;
    }
}

static CGContextRef juxu_voice_ui_current_cg_context(void)
{
    Class gcCls = objc_getClass("NSGraphicsContext");
    if (gcCls == Nil) return NULL;
    typedef id (*cur_fn)(id, SEL);
    typedef CGContextRef (*cg_fn)(id, SEL);
    id ctx = ((cur_fn)objc_msgSend)((id)gcCls, sel_registerName("currentContext"));
    if (ctx == nil) return NULL;
    return ((cg_fn)objc_msgSend)(ctx, sel_registerName("CGContext"));
}

void juxu_voice_ui_clip_save_rounded(double x, double y, double w, double h, double radius)
{
    CGContextRef cg = juxu_voice_ui_current_cg_context();
    if (cg == NULL) return;
    CGContextSaveGState(cg);
    if (w > 0.0 && h > 0.0) {
        double max_r = 0.5 * (w < h ? w : h);
        double r = radius < 0.0 ? 0.0 : (radius > max_r ? max_r : radius);
        CGPathRef path = CGPathCreateWithRoundedRect(CGRectMake(x, y, w, h), r, r, NULL);
        if (path != NULL) {
            CGContextAddPath(cg, path);
            CGContextClip(cg);
            CGPathRelease(path);
        }
    }
}

void juxu_voice_ui_clip_restore(void)
{
    CGContextRef cg = juxu_voice_ui_current_cg_context();
    if (cg != NULL) {
        CGContextRestoreGState(cg);
    }
}

void juxu_voice_ui_draw_spoke(double cx, double cy, double angle_rad, double r, double g, double b, double a)
{
    CGContextRef cg = juxu_voice_ui_current_cg_context();
    if (cg == NULL || a <= 0.0) return;
    CGContextSaveGState(cg);
    CGContextTranslateCTM(cg, cx, cy);
    CGContextRotateCTM(cg, angle_rad);
    CGColorRef col = CGColorCreateSRGB(r, g, b, a);
    if (col != NULL) {
        CGContextSetFillColorWithColor(cg, col);
        CGPathRef path = CGPathCreateWithRoundedRect(CGRectMake(-0.9, -1.9, 1.8, 3.8), 0.9, 0.9, NULL);
        if (path != NULL) {
            CGContextAddPath(cg, path);
            CGContextFillPath(cg);
            CGPathRelease(path);
        }
        CGColorRelease(col);
    }
    CGContextRestoreGState(cg);
}

bool juxu_voice_ui_open_url_utf8(const char *url_cstr)
{
    if (url_cstr == NULL || url_cstr[0] == '\0') return false;
    Class strCls = objc_getClass("NSString");
    Class urlCls = objc_getClass("NSURL");
    Class wsCls = objc_getClass("NSWorkspace");
    if (strCls == Nil || urlCls == Nil || wsCls == Nil) return false;
    typedef id (*str_fn)(id, SEL, const char *);
    typedef id (*url_fn)(id, SEL, id);
    typedef id (*ws_fn)(id, SEL);
    typedef bool (*open_fn)(id, SEL, id);
    id s = ((str_fn)objc_msgSend)((id)strCls, sel_registerName("stringWithUTF8String:"), url_cstr);
    if (s == nil) return false;
    id u = ((url_fn)objc_msgSend)((id)urlCls, sel_registerName("URLWithString:"), s);
    if (u == nil) return false;
    id ws = ((ws_fn)objc_msgSend)((id)wsCls, sel_registerName("sharedWorkspace"));
    if (ws == nil) return false;
    return ((open_fn)objc_msgSend)(ws, sel_registerName("openURL:"), u);
}

void juxu_voice_ui_copy_pasteboard_utf8(const char *utf8_cstr)
{
    if (utf8_cstr == NULL) utf8_cstr = "";
    Class pbCls = objc_getClass("NSPasteboard");
    Class strCls = objc_getClass("NSString");
    if (pbCls == Nil || strCls == Nil) return;
    typedef id (*gen_fn)(id, SEL);
    typedef long (*clr_fn)(id, SEL);
    typedef id (*str_fn)(id, SEL, const char *);
    typedef bool (*set_fn)(id, SEL, id, id);
    id pb = ((gen_fn)objc_msgSend)((id)pbCls, sel_registerName("generalPasteboard"));
    if (pb == nil) return;
    (void)((clr_fn)objc_msgSend)(pb, sel_registerName("clearContents"));
    id val = ((str_fn)objc_msgSend)((id)strCls, sel_registerName("stringWithUTF8String:"), utf8_cstr);
    id ptype = ((str_fn)objc_msgSend)((id)strCls, sel_registerName("stringWithUTF8String:"), "public.utf8-plain-text");
    if (val != nil && ptype != nil) {
        (void)((set_fn)objc_msgSend)(pb, sel_registerName("setString:forType:"), val, ptype);
    }
}

/* ---- True Zeroing-Weak CADisplayLink Proxy (`DisplayLinkWeakProxy`) ---- */

typedef void (*juxu_voice_display_step_fn)(void *view_ptr, double timestamp);

typedef struct JuxuWeakProxyBox {
    id weak_view;
    juxu_voice_display_step_fn step_cb;
} JuxuWeakProxyBox;

static char kWeakProxyBoxAssocKey = 0;
static Class gWeakProxyClass = Nil;

static void juxu_weak_proxy_step(id self, SEL _cmd, id link)
{
    (void)_cmd;
    void *raw = objc_getAssociatedObject(self, &kWeakProxyBoxAssocKey);
    if (raw == NULL) {
        if (link != nil) {
            ((void (*)(id, SEL))objc_msgSend)(link, sel_registerName("invalidate"));
        }
        return;
    }
    Class valCls = objc_getClass("NSValue");
    if (valCls == Nil) return;
    typedef void *(*ptr_fn)(id, SEL);
    JuxuWeakProxyBox *box = (JuxuWeakProxyBox *)((ptr_fn)objc_msgSend)((id)raw, sel_registerName("pointerValue"));
    if (box == NULL) return;

    id owner = objc_loadWeak(&box->weak_view);
    if (owner != nil) {
        double ts = 0.0;
        if (link != nil && ((bool (*)(id, SEL, SEL))objc_msgSend)(link, sel_registerName("respondsToSelector:"), sel_registerName("timestamp"))) {
            ts = ((double (*)(id, SEL))objc_msgSend)(link, sel_registerName("timestamp"));
        } else {
            ts = (double)CFAbsoluteTimeGetCurrent();
        }
        if (box->step_cb != NULL) {
            box->step_cb((void *)owner, ts);
        }
    } else {
        if (link != nil) {
            ((void (*)(id, SEL))objc_msgSend)(link, sel_registerName("invalidate"));
        }
    }
}

static void juxu_weak_proxy_dealloc(id self, SEL _cmd)
{
    void *raw = objc_getAssociatedObject(self, &kWeakProxyBoxAssocKey);
    if (raw != NULL) {
        typedef void *(*ptr_fn)(id, SEL);
        JuxuWeakProxyBox *box = (JuxuWeakProxyBox *)((ptr_fn)objc_msgSend)((id)raw, sel_registerName("pointerValue"));
        if (box != NULL) {
            objc_storeWeak(&box->weak_view, nil);
            free(box);
        }
    }
    Class supCls = objc_getClass("NSObject");
    struct objc_super sup = { self, supCls };
    ((void (*)(struct objc_super *, SEL))objc_msgSendSuper)(&sup, _cmd);
}

static Class juxu_ensure_weak_proxy_class(void)
{
    if (gWeakProxyClass != Nil) return gWeakProxyClass;
    Class existing = objc_getClass("JuxuGeishaVoiceDisplayLinkProxy");
    if (existing != Nil) {
        gWeakProxyClass = existing;
        return existing;
    }
    Class nsObj = objc_getClass("NSObject");
    if (nsObj == Nil) return Nil;
    Class cls = objc_allocateClassPair(nsObj, "JuxuGeishaVoiceDisplayLinkProxy", 0);
    if (cls != Nil) {
        class_addMethod(cls, sel_registerName("step:"), (IMP)juxu_weak_proxy_step, "v@:@");
        class_addMethod(cls, sel_registerName("dealloc"), (IMP)juxu_weak_proxy_dealloc, "v@:");
        objc_registerClassPair(cls);
        gWeakProxyClass = cls;
    }
    return gWeakProxyClass;
}

typedef struct JuxuCAFrameRateRange {
    float minimum;
    float maximum;
    float preferred;
} JuxuCAFrameRateRange;

void *juxu_voice_ui_start_display_link(void *view_ptr, void *step_fn_ptr)
{
    if (view_ptr == NULL || step_fn_ptr == NULL) return NULL;
    Class proxyCls = juxu_ensure_weak_proxy_class();
    Class valCls = objc_getClass("NSValue");
    Class rlCls = objc_getClass("NSRunLoop");
    Class strCls = objc_getClass("NSString");
    if (proxyCls == Nil || valCls == Nil || rlCls == Nil || strCls == Nil) return NULL;

    id proxy = ((id (*)(id, SEL))objc_msgSend)(
        ((id (*)(id, SEL))objc_msgSend)((id)proxyCls, sel_registerName("alloc")),
        sel_registerName("init")
    );
    if (proxy == nil) return NULL;

    JuxuWeakProxyBox *box = (JuxuWeakProxyBox *)calloc(1, sizeof(JuxuWeakProxyBox));
    objc_storeWeak(&box->weak_view, (id)view_ptr);
    box->step_cb = (juxu_voice_display_step_fn)step_fn_ptr;

    id valObj = ((id (*)(id, SEL, const void *))objc_msgSend)((id)valCls, sel_registerName("valueWithPointer:"), box);
    objc_setAssociatedObject(proxy, &kWeakProxyBoxAssocKey, valObj, OBJC_ASSOCIATION_RETAIN_NONATOMIC);

    id view = (id)view_ptr;
    SEL dlSel = sel_registerName("displayLinkWithTarget:selector:");
    id handle = nil;
    id mainRl = ((id (*)(id, SEL))objc_msgSend)((id)rlCls, sel_registerName("mainRunLoop"));
    id commonMode = ((id (*)(id, SEL, const char *))objc_msgSend)((id)strCls, sel_registerName("stringWithUTF8String:"), "kCFRunLoopCommonModes");

    if (((bool (*)(id, SEL, SEL))objc_msgSend)(view, sel_registerName("respondsToSelector:"), dlSel)) {
        handle = ((id (*)(id, SEL, id, SEL))objc_msgSend)(view, dlSel, proxy, sel_registerName("step:"));
        if (handle != nil) {
            SEL frSel = sel_registerName("setPreferredFrameRateRange:");
            if (((bool (*)(id, SEL, SEL))objc_msgSend)(handle, sel_registerName("respondsToSelector:"), frSel)) {
                JuxuCAFrameRateRange fr = { 30.0f, 60.0f, 60.0f };
                ((void (*)(id, SEL, JuxuCAFrameRateRange))objc_msgSend)(handle, frSel, fr);
            }
            ((void (*)(id, SEL, id, id))objc_msgSend)(handle, sel_registerName("addToRunLoop:forMode:"), mainRl, commonMode);
        }
    }
    if (handle == nil) {
        Class timerCls = objc_getClass("NSTimer");
        if (timerCls != Nil) {
            handle = ((id (*)(id, SEL, double, id, SEL, id, bool))objc_msgSend)(
                (id)timerCls,
                sel_registerName("timerWithTimeInterval:target:selector:userInfo:repeats:"),
                1.0 / 60.0,
                proxy,
                sel_registerName("step:"),
                nil,
                true
            );
            if (handle != nil) {
                ((void (*)(id, SEL, id, id))objc_msgSend)(mainRl, sel_registerName("addTimer:forMode:"), handle, commonMode);
            }
        }
    }

    /* Release local retain on proxy; CADisplayLink / NSTimer retains proxy, and proxy holds a weak reference to view! */
    ((void (*)(id, SEL))objc_msgSend)(proxy, sel_registerName("release"));
    return (void *)handle;
}

void juxu_voice_ui_stop_display_link(void *handle_ptr)
{
    if (handle_ptr != NULL) {
        ((void (*)(id, SEL))objc_msgSend)((id)handle_ptr, sel_registerName("invalidate"));
    }
}

void *juxu_voice_ui_weak_box_new(void *obj_ptr)
{
    id *slot = (id *)calloc(1, sizeof(id));
    if (slot == NULL) return NULL;
    if (obj_ptr != NULL) {
        objc_storeWeak(slot, (id)obj_ptr);
    }
    return slot;
}

bool juxu_voice_ui_weak_box_is_alive(void *slot_ptr)
{
    if (slot_ptr == NULL) return false;
    id loaded = objc_loadWeak((id *)slot_ptr);
    return loaded != nil;
}

void juxu_voice_ui_weak_box_free(void *slot_ptr)
{
    if (slot_ptr != NULL) {
        objc_storeWeak((id *)slot_ptr, nil);
        free(slot_ptr);
    }
}

#else
/* Non-Apple stubs */
void *juxu_voice_f32_buf_alloc(int32_t count) { return calloc((size_t)(count > 0 ? count : 1), sizeof(float)); }
void juxu_voice_f32_buf_free(void *buf) { free(buf); }
void juxu_voice_f32_buf_set(void *buf, int32_t idx, float val) { if (buf && idx >= 0) ((float *)buf)[idx] = val; }
float juxu_voice_f32_buf_get(const void *buf, int32_t idx) { return (buf && idx >= 0) ? ((const float *)buf)[idx] : 0.0f; }
void *juxu_voice_spectrum_new(void) { return NULL; }
void juxu_voice_spectrum_free(void *ptr) { (void)ptr; }
int32_t juxu_voice_spectrum_frame_cap(int32_t c, double sr) { (void)c; (void)sr; return 0; }
int32_t juxu_voice_spectrum_process(void *p, const float *s, int32_t c, double sr, float *o, int32_t m) { (void)p; (void)s; (void)c; (void)sr; (void)o; (void)m; return 0; }
void juxu_voice_spectrum_process_frame(void *p, const float *s, int32_t c, double sr, bool h, float sec, float *o) { (void)p; (void)s; (void)c; (void)sr; (void)h; (void)sec; if (o) { for (int i = 0; i < 11; i++) o[i] = 0.0f; } }
int64_t juxu_voice_ui_floating_window_level(void) { return 5; }
bool juxu_voice_ui_reduce_motion(void) { return false; }
void juxu_voice_ui_layer_continuous_corner(void *l, double r, bool m) { (void)l; (void)r; (void)m; }
void juxu_voice_ui_layer_border_rgba(void *l, double w, double r, double g, double b, double a) { (void)l; (void)w; (void)r; (void)g; (void)b; (void)a; }
void juxu_voice_ui_bounding_rect(void *s, double w, void *a, double *o) { (void)s; (void)w; (void)a; if (o) { o[0]=0; o[1]=0; o[2]=0; o[3]=0; } }
void juxu_voice_ui_clip_save_rounded(double x, double y, double w, double h, double r) { (void)x; (void)y; (void)w; (void)h; (void)r; }
void juxu_voice_ui_clip_restore(void) {}
void juxu_voice_ui_draw_spoke(double cx, double cy, double ang, double r, double g, double b, double a) { (void)cx; (void)cy; (void)ang; (void)r; (void)g; (void)b; (void)a; }
bool juxu_voice_ui_open_url_utf8(const char *u) { (void)u; return false; }
void juxu_voice_ui_copy_pasteboard_utf8(const char *s) { (void)s; }
void *juxu_voice_ui_start_display_link(void *v, void *f) { (void)v; (void)f; return NULL; }
void juxu_voice_ui_stop_display_link(void *h) { (void)h; }
void *juxu_voice_ui_weak_box_new(void *o) { (void)o; return NULL; }
bool juxu_voice_ui_weak_box_is_alive(void *s) { (void)s; return false; }
void juxu_voice_ui_weak_box_free(void *s) { (void)s; }
#endif
