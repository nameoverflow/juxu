/*
 * Pure C11 iOS UIKit / QuartzCore / CoreHaptics ABI Bridge (`ios-keyboard/csrc/ios_ui_abi.c`).
 * Compiles cleanly under `-fapplication-extension` on `ios-app` & `ios-sim-app`,
 * and compiles to safe no-op stubs on `macos` (`geisha test`).
 */

#include <math.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

extern void gc_handle_free(int64_t handle);

#if defined(__APPLE__)
#include <TargetConditionals.h>
#endif

#if defined(__APPLE__) && TARGET_OS_IOS

#include <CoreGraphics/CoreGraphics.h>
#include <dispatch/dispatch.h>
#include <objc/message.h>
#include <objc/objc.h>
#include <objc/runtime.h>

/* ---- Apple SDK Extern Constants (QuartzCore / UIKit / Foundation / CoreHaptics) ---- */

extern id const kCACornerCurveContinuous;
extern uint64_t const UIAccessibilityTraitButton;

extern double const UIFontWeightRegular;
extern double const UIFontWeightMedium;
extern double const UIFontWeightSemibold;
extern double const UIFontWeightBold;

extern id const NSFontAttributeName;
extern id const NSForegroundColorAttributeName;
extern id const NSParagraphStyleAttributeName;
extern id const NSRunLoopCommonModes;

extern id const UITextContentTypeOneTimeCode;
extern id const UITextContentTypePassword;
extern id const UITextContentTypeNewPassword;
extern id const UITextContentTypeCreditCardNumber;
extern id const UITextContentTypeTelephoneNumber;
extern id const UITextContentTypeEmailAddress;
extern id const UITextContentTypeURL;
extern id const UITextContentTypeUsername;

extern id const UIPasteboardChangedNotification;

extern id const CHHapticEventTypeHapticTransient;
extern id const CHHapticEventParameterIDHapticIntensity;
extern id const CHHapticEventParameterIDHapticSharpness;

extern void *_NSConcreteStackBlock[32];
extern void *_Block_copy(const void *aBlock);
extern void _Block_release(const void *aBlock);

typedef struct juxu_ios_simple_desc {
    unsigned long reserved;
    unsigned long size;
} juxu_ios_simple_desc_t;

/* ---- Block Invokers for Geisha Closures ---- */

typedef struct juxu_ios_blk0 {
    void *isa;
    int flags;
    int reserved;
    void *invoke;
    void *descriptor;
    void (*tramp)(int64_t);
    int64_t handle;
    bool free_on_call;
} juxu_ios_blk0_t;

static juxu_ios_simple_desc_t g_blk0_desc = { 0, sizeof(juxu_ios_blk0_t) };

static void juxu_ios_blk0_invoke(juxu_ios_blk0_t *b)
{
    if (b->tramp != NULL) {
        b->tramp(b->handle);
    }
    if (b->free_on_call && b->handle != 0) {
        int64_t h = b->handle;
        b->handle = 0;
        gc_handle_free(h);
    }
}

typedef struct juxu_ios_blk_bool {
    void *isa;
    int flags;
    int reserved;
    void *invoke;
    void *descriptor;
    void (*tramp)(int64_t, bool);
    int64_t handle;
} juxu_ios_blk_bool_t;

static juxu_ios_simple_desc_t g_blk_bool_desc = { 0, sizeof(juxu_ios_blk_bool_t) };

static void juxu_ios_blk_bool_invoke(juxu_ios_blk_bool_t *b, bool finished)
{
    if (b->tramp != NULL) {
        b->tramp(b->handle, finished);
    }
    if (b->handle != 0) {
        int64_t h = b->handle;
        b->handle = 0;
        gc_handle_free(h);
    }
}

typedef struct juxu_ios_trait_blk {
    void *isa;
    int flags;
    int reserved;
    void *invoke;
    void *descriptor;
    void (*tramp)(int64_t);
    int64_t handle;
} juxu_ios_trait_blk_t;

static juxu_ios_simple_desc_t g_trait_blk_desc = { 0, sizeof(juxu_ios_trait_blk_t) };

static void juxu_ios_trait_blk_invoke(juxu_ios_trait_blk_t *b, id traitEnv, id prevTraits)
{
    (void)traitEnv;
    (void)prevTraits;
    if (b->tramp != NULL) {
        b->tramp(b->handle);
    }
}

/* ---- QuartzCore / CALayer Helpers ---- */

void juxu_ios_set_continuous_corner(void *layer_ptr, double radius)
{
    id layer = (id)layer_ptr;
    if (layer == nil) return;
    ((void (*)(id, SEL, double))objc_msgSend)(layer, sel_registerName("setCornerRadius:"), radius);
    ((void (*)(id, SEL, id))objc_msgSend)(layer, sel_registerName("setCornerCurve:"), kCACornerCurveContinuous);
}

void juxu_ios_set_top_masked_corners(void *layer_ptr)
{
    id layer = (id)layer_ptr;
    if (layer == nil) return;
    /* kCALayerMinXMinYCorner (1) | kCALayerMaxXMinYCorner (2) == 3 */
    ((void (*)(id, SEL, uint64_t))objc_msgSend)(layer, sel_registerName("setMaskedCorners:"), 3ULL);
}

void juxu_ios_catransaction_begin_no_actions(void)
{
    Class cls = objc_getClass("CATransaction");
    if (cls == Nil) return;
    ((void (*)(id, SEL))objc_msgSend)((id)cls, sel_registerName("begin"));
    ((void (*)(id, SEL, bool))objc_msgSend)((id)cls, sel_registerName("setDisableActions:"), true);
}

void juxu_ios_catransaction_commit(void)
{
    Class cls = objc_getClass("CATransaction");
    if (cls == Nil) return;
    ((void (*)(id, SEL))objc_msgSend)((id)cls, sel_registerName("commit"));
}

static id make_nsstring_utf8(const char *s)
{
    Class cls = objc_getClass("NSString");
    if (cls == Nil) return nil;
    return ((id (*)(id, SEL, const char *))objc_msgSend)((id)cls, sel_registerName("stringWithUTF8String:"), s ? s : "");
}

static id make_array_objects(const id *objs, uint64_t count)
{
    Class cls = objc_getClass("NSArray");
    if (cls == Nil) return nil;
    return ((id (*)(id, SEL, const id *, uint64_t))objc_msgSend)((id)cls, sel_registerName("arrayWithObjects:count:"), objs, count);
}

static id make_dict_objects_keys(const id *objs, const id *keys, uint64_t count)
{
    Class cls = objc_getClass("NSDictionary");
    if (cls == Nil) return nil;
    return ((id (*)(id, SEL, const id *, const id *, uint64_t))objc_msgSend)(
        (id)cls, sel_registerName("dictionaryWithObjects:forKeys:count:"), objs, keys, count
    );
}

void juxu_ios_init_edge_fade_layer(void *grad_layer_ptr)
{
    id grad = (id)grad_layer_ptr;
    if (grad == nil) return;
    Class colCls = objc_getClass("UIColor");
    Class numCls = objc_getClass("NSNumber");
    id blackCol = ((id (*)(id, SEL))objc_msgSend)((id)colCls, sel_registerName("blackColor"));
    id clearCol = ((id (*)(id, SEL))objc_msgSend)((id)colCls, sel_registerName("clearColor"));
    id solidCg = ((id (*)(id, SEL))objc_msgSend)(blackCol, sel_registerName("CGColor"));
    id clearCg = ((id (*)(id, SEL))objc_msgSend)(clearCol, sel_registerName("CGColor"));
    id cols[3] = { solidCg, solidCg, clearCg };
    id locs[3] = {
        ((id (*)(id, SEL, double))objc_msgSend)((id)numCls, sel_registerName("numberWithDouble:"), 0.0),
        ((id (*)(id, SEL, double))objc_msgSend)((id)numCls, sel_registerName("numberWithDouble:"), 0.91),
        ((id (*)(id, SEL, double))objc_msgSend)((id)numCls, sel_registerName("numberWithDouble:"), 1.0)
    };
    ((void (*)(id, SEL, id))objc_msgSend)(grad, sel_registerName("setColors:"), make_array_objects(cols, 3));
    ((void (*)(id, SEL, id))objc_msgSend)(grad, sel_registerName("setLocations:"), make_array_objects(locs, 3));
    ((void (*)(id, SEL, CGPoint))objc_msgSend)(grad, sel_registerName("setStartPoint:"), CGPointMake(0.0, 0.5));
    ((void (*)(id, SEL, CGPoint))objc_msgSend)(grad, sel_registerName("setEndPoint:"), CGPointMake(1.0, 0.5));
}

void juxu_ios_apply_edge_fade_mask(
    void *grad_layer_ptr,
    void *clip_view_ptr,
    bool has_left,
    bool has_right,
    double avail_w,
    double avail_h
)
{
    id grad = (id)grad_layer_ptr;
    id clipView = (id)clip_view_ptr;
    if (grad == nil || clipView == nil) return;
    id clipLayer = ((id (*)(id, SEL))objc_msgSend)(clipView, sel_registerName("layer"));
    if (clipLayer == nil) return;

    juxu_ios_catransaction_begin_no_actions();
    if ((has_left || has_right) && avail_w > 0.0) {
        double fade = 18.0 / avail_w;
        if (fade > 0.25) fade = 0.25;
        Class colCls = objc_getClass("UIColor");
        Class numCls = objc_getClass("NSNumber");
        id blackCol = ((id (*)(id, SEL))objc_msgSend)((id)colCls, sel_registerName("blackColor"));
        id clearCol = ((id (*)(id, SEL))objc_msgSend)((id)colCls, sel_registerName("clearColor"));
        id solidCg = ((id (*)(id, SEL))objc_msgSend)(blackCol, sel_registerName("CGColor"));
        id clearCg = ((id (*)(id, SEL))objc_msgSend)(clearCol, sel_registerName("CGColor"));

        ((void (*)(id, SEL, CGRect))objc_msgSend)(grad, sel_registerName("setFrame:"), CGRectMake(0.0, 0.0, avail_w, avail_h));
        id cols[4] = {
            has_left ? clearCg : solidCg,
            solidCg,
            solidCg,
            has_right ? clearCg : solidCg
        };
        id locs[4] = {
            ((id (*)(id, SEL, double))objc_msgSend)((id)numCls, sel_registerName("numberWithDouble:"), 0.0),
            ((id (*)(id, SEL, double))objc_msgSend)((id)numCls, sel_registerName("numberWithDouble:"), has_left ? fade : 0.0),
            ((id (*)(id, SEL, double))objc_msgSend)((id)numCls, sel_registerName("numberWithDouble:"), has_right ? (1.0 - fade) : 1.0),
            ((id (*)(id, SEL, double))objc_msgSend)((id)numCls, sel_registerName("numberWithDouble:"), 1.0)
        };
        ((void (*)(id, SEL, id))objc_msgSend)(grad, sel_registerName("setColors:"), make_array_objects(cols, 4));
        ((void (*)(id, SEL, id))objc_msgSend)(grad, sel_registerName("setLocations:"), make_array_objects(locs, 4));
        id curMask = ((id (*)(id, SEL))objc_msgSend)(clipLayer, sel_registerName("mask"));
        if (curMask != grad) {
            ((void (*)(id, SEL, id))objc_msgSend)(clipLayer, sel_registerName("setMask:"), grad);
        }
    } else {
        id curMask = ((id (*)(id, SEL))objc_msgSend)(clipLayer, sel_registerName("mask"));
        if (curMask != nil) {
            ((void (*)(id, SEL, id))objc_msgSend)(clipLayer, sel_registerName("setMask:"), nil);
        }
    }
    juxu_ios_catransaction_commit();
}

/* ---- UIKit Colors, Fonts, SF Symbols & Attributed Strings ---- */

void *juxu_ios_color_rgba(double r, double g, double b, double a)
{
    Class cls = objc_getClass("UIColor");
    if (cls == Nil) return NULL;
    return (void *)((id (*)(id, SEL, double, double, double, double))objc_msgSend)(
        (id)cls, sel_registerName("colorWithRed:green:blue:alpha:"), r, g, b, a
    );
}

void *juxu_ios_color_white(double w, double a)
{
    Class cls = objc_getClass("UIColor");
    if (cls == Nil) return NULL;
    return (void *)((id (*)(id, SEL, double, double))objc_msgSend)(
        (id)cls, sel_registerName("colorWithWhite:alpha:"), w, a
    );
}

void *juxu_ios_color_system_blue(double alpha)
{
    Class cls = objc_getClass("UIColor");
    if (cls == Nil) return NULL;
    id blue = ((id (*)(id, SEL))objc_msgSend)((id)cls, sel_registerName("systemBlueColor"));
    if (alpha >= 0.999) return (void *)blue;
    return (void *)((id (*)(id, SEL, double))objc_msgSend)(blue, sel_registerName("colorWithAlphaComponent:"), alpha);
}

void *juxu_ios_color_system_bg_alpha(double alpha)
{
    Class cls = objc_getClass("UIColor");
    if (cls == Nil) return NULL;
    id bg = ((id (*)(id, SEL))objc_msgSend)((id)cls, sel_registerName("systemBackgroundColor"));
    return (void *)((id (*)(id, SEL, double))objc_msgSend)(bg, sel_registerName("colorWithAlphaComponent:"), alpha);
}

void *juxu_ios_color_label_alpha(double alpha)
{
    Class cls = objc_getClass("UIColor");
    if (cls == Nil) return NULL;
    id lbl = ((id (*)(id, SEL))objc_msgSend)((id)cls, sel_registerName("labelColor"));
    return (void *)((id (*)(id, SEL, double))objc_msgSend)(lbl, sel_registerName("colorWithAlphaComponent:"), alpha);
}

void *juxu_ios_cgcolor(void *ui_color)
{
    if (ui_color == NULL) return NULL;
    return (void *)((id (*)(id, SEL))objc_msgSend)((id)ui_color, sel_registerName("CGColor"));
}

static double decode_font_weight(int weight_code)
{
    switch (weight_code) {
    case 1: return UIFontWeightMedium;
    case 2: return UIFontWeightSemibold;
    case 3: return UIFontWeightBold;
    default: return UIFontWeightRegular;
    }
}

void *juxu_ios_system_font(double size, int weight_code)
{
    Class cls = objc_getClass("UIFont");
    if (cls == Nil) return NULL;
    return (void *)((id (*)(id, SEL, double, double))objc_msgSend)(
        (id)cls, sel_registerName("systemFontOfSize:weight:"), size, decode_font_weight(weight_code)
    );
}

void *juxu_ios_monospaced_font(double size, int weight_code)
{
    Class cls = objc_getClass("UIFont");
    if (cls == Nil) return NULL;
    return (void *)((id (*)(id, SEL, double, double))objc_msgSend)(
        (id)cls, sel_registerName("monospacedSystemFontOfSize:weight:"), size, decode_font_weight(weight_code)
    );
}

void *juxu_ios_monospaced_digit_font(double size, int weight_code)
{
    Class cls = objc_getClass("UIFont");
    if (cls == Nil) return NULL;
    return (void *)((id (*)(id, SEL, double, double))objc_msgSend)(
        (id)cls, sel_registerName("monospacedDigitSystemFontOfSize:weight:"), size, decode_font_weight(weight_code)
    );
}

/* weight_code: -1 = unconfigured systemImageNamed:, 0 = Regular(4), 1 = Medium(5), 2 = Semibold(6) */
void *juxu_ios_symbol_image(const char *name, double point_size, int weight_code, bool always_template)
{
    if (name == NULL || name[0] == '\0') return NULL;
    Class imgCls = objc_getClass("UIImage");
    if (imgCls == Nil) return NULL;
    id nsName = make_nsstring_utf8(name);
    id img = nil;
    if (weight_code < 0) {
        img = ((id (*)(id, SEL, id))objc_msgSend)((id)imgCls, sel_registerName("systemImageNamed:"), nsName);
    } else {
        int64_t symWeight = 4; /* UIImageSymbolWeightRegular == 4 */
        if (weight_code == 1) symWeight = 5;      /* Medium */
        else if (weight_code == 2) symWeight = 6; /* Semibold */
        Class cfgCls = objc_getClass("UIImageSymbolConfiguration");
        id cfg = ((id (*)(id, SEL, double, int64_t))objc_msgSend)(
            (id)cfgCls, sel_registerName("configurationWithPointSize:weight:"), point_size, symWeight
        );
        img = ((id (*)(id, SEL, id, id))objc_msgSend)(
            (id)imgCls, sel_registerName("systemImageNamed:withConfiguration:"), nsName, cfg
        );
    }
    if (img != nil && always_template) {
        /* UIImageRenderingModeAlwaysTemplate == 2 */
        img = ((id (*)(id, SEL, int64_t))objc_msgSend)(img, sel_registerName("imageWithRenderingMode:"), 2LL);
    }
    return (void *)img;
}

double juxu_ios_measure_text_width(const char *text, void *ui_font)
{
    if (text == NULL || text[0] == '\0' || ui_font == NULL) return 0.0;
    id ns = make_nsstring_utf8(text);
    id objs[1] = { (id)ui_font };
    id keys[1] = { NSFontAttributeName };
    id attrs = make_dict_objects_keys(objs, keys, 1);
    CGSize sz = ((CGSize (*)(id, SEL, id))objc_msgSend)(ns, sel_registerName("sizeWithAttributes:"), attrs);
    return sz.width;
}

double juxu_ios_set_dual_attributed_label(
    void *ui_label,
    const char *primary,
    void *primary_font,
    void *primary_color,
    const char *secondary,
    void *secondary_font,
    void *secondary_color
)
{
    if (ui_label == NULL || primary_font == NULL || primary_color == NULL) return 0.0;
    Class mutAttrCls = objc_getClass("NSMutableAttributedString");
    Class attrCls = objc_getClass("NSAttributedString");
    if (mutAttrCls == Nil || attrCls == Nil) return 0.0;

    id pObjs[2] = { (id)primary_font, (id)primary_color };
    id pKeys[2] = { NSFontAttributeName, NSForegroundColorAttributeName };
    id pDict = make_dict_objects_keys(pObjs, pKeys, 2);

    id allocMut = ((id (*)(id, SEL))objc_msgSend)((id)mutAttrCls, sel_registerName("alloc"));
    id mut = ((id (*)(id, SEL, id, id))objc_msgSend)(
        allocMut, sel_registerName("initWithString:attributes:"), make_nsstring_utf8(primary), pDict
    );

    if (secondary != NULL && secondary[0] != '\0' && secondary_font != NULL && secondary_color != NULL) {
        id sObjs[2] = { (id)secondary_font, (id)secondary_color };
        id sKeys[2] = { NSFontAttributeName, NSForegroundColorAttributeName };
        id sDict = make_dict_objects_keys(sObjs, sKeys, 2);
        id allocSub = ((id (*)(id, SEL))objc_msgSend)((id)attrCls, sel_registerName("alloc"));
        id sub = ((id (*)(id, SEL, id, id))objc_msgSend)(
            allocSub, sel_registerName("initWithString:attributes:"), make_nsstring_utf8(secondary), sDict
        );
        ((void (*)(id, SEL, id))objc_msgSend)(mut, sel_registerName("appendAttributedString:"), sub);
        ((void (*)(id, SEL))objc_msgSend)(sub, sel_registerName("release"));
    }

    ((void (*)(id, SEL, id))objc_msgSend)((id)ui_label, sel_registerName("setAttributedText:"), mut);
    CGSize sz = ((CGSize (*)(id, SEL))objc_msgSend)(mut, sel_registerName("size"));
    ((void (*)(id, SEL))objc_msgSend)(mut, sel_registerName("release"));
    return ceil(sz.width);
}

void juxu_ios_set_radical_attributed_label(
    void *ui_label,
    const char *radicals,
    double font_size,
    void *text_color,
    double line_height_multiple,
    double line_spacing
)
{
    if (ui_label == NULL || text_color == NULL) return;
    Class paraCls = objc_getClass("NSMutableParagraphStyle");
    Class attrCls = objc_getClass("NSAttributedString");
    if (paraCls == Nil || attrCls == Nil) return;

    id para = ((id (*)(id, SEL))objc_msgSend)(
        ((id (*)(id, SEL))objc_msgSend)((id)paraCls, sel_registerName("alloc")),
        sel_registerName("init")
    );
    /* NSTextAlignmentCenter == 1 */
    ((void (*)(id, SEL, int64_t))objc_msgSend)(para, sel_registerName("setAlignment:"), 1LL);
    ((void (*)(id, SEL, double))objc_msgSend)(para, sel_registerName("setLineHeightMultiple:"), line_height_multiple);
    ((void (*)(id, SEL, double))objc_msgSend)(para, sel_registerName("setLineSpacing:"), line_spacing);

    id font = (id)juxu_ios_system_font(font_size, 0);
    id objs[3] = { font, (id)text_color, para };
    id keys[3] = { NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName };
    id dict = make_dict_objects_keys(objs, keys, 3);

    id attr = ((id (*)(id, SEL, id, id))objc_msgSend)(
        ((id (*)(id, SEL))objc_msgSend)((id)attrCls, sel_registerName("alloc")),
        sel_registerName("initWithString:attributes:"),
        make_nsstring_utf8(radicals),
        dict
    );
    ((void (*)(id, SEL, id))objc_msgSend)((id)ui_label, sel_registerName("setAttributedText:"), attr);
    ((void (*)(id, SEL))objc_msgSend)(attr, sel_registerName("release"));
    ((void (*)(id, SEL))objc_msgSend)(para, sel_registerName("release"));
}

/* ---- UIView Animations ---- */

void juxu_ios_perform_without_animation(void *trampoline, int64_t gc_handle)
{
    Class viewCls = objc_getClass("UIView");
    if (viewCls == Nil) {
        if (trampoline != NULL) ((void (*)(int64_t))trampoline)(gc_handle);
        if (gc_handle != 0) gc_handle_free(gc_handle);
        return;
    }
    juxu_ios_blk0_t blk = {
        _NSConcreteStackBlock, 0, 0,
        (void *)juxu_ios_blk0_invoke,
        &g_blk0_desc,
        (void (*)(int64_t))trampoline,
        gc_handle,
        false
    };
    ((void (*)(id, SEL, void *))objc_msgSend)((id)viewCls, sel_registerName("performWithoutAnimation:"), &blk);
    if (gc_handle != 0) {
        gc_handle_free(gc_handle);
    }
}

void juxu_ios_animate(
    double duration,
    uint64_t options,
    void *anim_tramp,
    int64_t anim_handle,
    void *comp_tramp,
    int64_t comp_handle
)
{
    Class viewCls = objc_getClass("UIView");
    if (viewCls == Nil) {
        if (anim_tramp != NULL) ((void (*)(int64_t))anim_tramp)(anim_handle);
        if (anim_handle != 0) gc_handle_free(anim_handle);
        if (comp_tramp != NULL) ((void (*)(int64_t, bool))comp_tramp)(comp_handle, true);
        if (comp_handle != 0) gc_handle_free(comp_handle);
        return;
    }
    juxu_ios_blk0_t stack_anim = {
        _NSConcreteStackBlock, 0, 0,
        (void *)juxu_ios_blk0_invoke,
        &g_blk0_desc,
        (void (*)(int64_t))anim_tramp,
        anim_handle,
        true
    };
    void *heap_anim = _Block_copy(&stack_anim);

    void *heap_comp = NULL;
    if (comp_tramp != NULL && comp_handle != 0) {
        juxu_ios_blk_bool_t stack_comp = {
            _NSConcreteStackBlock, 0, 0,
            (void *)juxu_ios_blk_bool_invoke,
            &g_blk_bool_desc,
            (void (*)(int64_t, bool))comp_tramp,
            comp_handle
        };
        heap_comp = _Block_copy(&stack_comp);
    } else if (comp_handle != 0) {
        gc_handle_free(comp_handle);
    }

    ((void (*)(id, SEL, double, double, uint64_t, void *, void *))objc_msgSend)(
        (id)viewCls,
        sel_registerName("animateWithDuration:delay:options:animations:completion:"),
        duration,
        0.0,
        options,
        heap_anim,
        heap_comp
    );
    _Block_release(heap_anim);
    if (heap_comp != NULL) _Block_release(heap_comp);
}

void juxu_ios_animate_spring(
    double duration,
    double damping,
    double velocity,
    uint64_t options,
    void *anim_tramp,
    int64_t anim_handle
)
{
    Class viewCls = objc_getClass("UIView");
    if (viewCls == Nil) {
        if (anim_tramp != NULL) ((void (*)(int64_t))anim_tramp)(anim_handle);
        if (anim_handle != 0) gc_handle_free(anim_handle);
        return;
    }
    juxu_ios_blk0_t stack_anim = {
        _NSConcreteStackBlock, 0, 0,
        (void *)juxu_ios_blk0_invoke,
        &g_blk0_desc,
        (void (*)(int64_t))anim_tramp,
        anim_handle,
        true
    };
    void *heap_anim = _Block_copy(&stack_anim);
    ((void (*)(id, SEL, double, double, double, double, uint64_t, void *, void *))objc_msgSend)(
        (id)viewCls,
        sel_registerName("animateWithDuration:delay:usingSpringWithDamping:initialSpringVelocity:options:animations:completion:"),
        duration,
        0.0,
        damping,
        velocity,
        options,
        heap_anim,
        NULL
    );
    _Block_release(heap_anim);
}

/* ---- Accessibility, Trait Registration, Proxy Inspection & Touch Sorting ---- */

uint64_t juxu_ios_accessibility_trait_button(void)
{
    return UIAccessibilityTraitButton;
}

void *juxu_ios_pasteboard_changed_notification_name(void)
{
    return (void *)UIPasteboardChangedNotification;
}

void juxu_ios_register_style_trait_observer(void *vc_ptr, void *trampoline, int64_t gc_handle)
{
    id vc = (id)vc_ptr;
    Class traitCls = objc_getClass("UITraitUserInterfaceStyle");
    SEL regSel = sel_registerName("registerForTraitChanges:withHandler:");
    if (vc == nil || traitCls == Nil || !((bool (*)(id, SEL, SEL))objc_msgSend)(vc, sel_registerName("respondsToSelector:"), regSel)) {
        if (gc_handle != 0) gc_handle_free(gc_handle);
        return;
    }
    id traits[1] = { (id)traitCls };
    id arr = make_array_objects(traits, 1);
    juxu_ios_trait_blk_t stack_blk = {
        _NSConcreteStackBlock, 0, 0,
        (void *)juxu_ios_trait_blk_invoke,
        &g_trait_blk_desc,
        (void (*)(int64_t))trampoline,
        gc_handle
    };
    void *heap_blk = _Block_copy(&stack_blk);
    ((id (*)(id, SEL, id, void *))objc_msgSend)(vc, regSel, arr, heap_blk);
    _Block_release(heap_blk);
}

static bool nsstr_eq(id a, id b)
{
    if (a == b) return true;
    if (a == nil || b == nil) return false;
    return ((bool (*)(id, SEL, id))objc_msgSend)(a, sel_registerName("isEqualToString:"), b);
}

bool juxu_ios_proxy_is_one_time_code(void *proxy_ptr)
{
    id proxy = (id)proxy_ptr;
    if (proxy == nil) return false;
    SEL sel = sel_registerName("textContentType");
    if (!((bool (*)(id, SEL, SEL))objc_msgSend)(proxy, sel_registerName("respondsToSelector:"), sel)) return false;
    id ct = ((id (*)(id, SEL))objc_msgSend)(proxy, sel);
    return nsstr_eq(ct, UITextContentTypeOneTimeCode);
}

bool juxu_ios_proxy_is_sensitive_eval(void *proxy_ptr)
{
    id proxy = (id)proxy_ptr;
    if (proxy == nil) return false;
    SEL secSel = sel_registerName("isSecureTextEntry");
    if (((bool (*)(id, SEL, SEL))objc_msgSend)(proxy, sel_registerName("respondsToSelector:"), secSel)) {
        if (((bool (*)(id, SEL))objc_msgSend)(proxy, secSel)) return true;
    }
    SEL ctSel = sel_registerName("textContentType");
    if (((bool (*)(id, SEL, SEL))objc_msgSend)(proxy, sel_registerName("respondsToSelector:"), ctSel)) {
        id ct = ((id (*)(id, SEL))objc_msgSend)(proxy, ctSel);
        if (ct != nil) {
            if (nsstr_eq(ct, UITextContentTypePassword)
                || nsstr_eq(ct, UITextContentTypeNewPassword)
                || nsstr_eq(ct, UITextContentTypeOneTimeCode)
                || nsstr_eq(ct, UITextContentTypeCreditCardNumber)
                || nsstr_eq(ct, UITextContentTypeTelephoneNumber)
                || nsstr_eq(ct, UITextContentTypeEmailAddress)
                || nsstr_eq(ct, UITextContentTypeURL)
                || nsstr_eq(ct, UITextContentTypeUsername)) {
                return true;
            }
        }
    }
    SEL kbSel = sel_registerName("keyboardType");
    if (((bool (*)(id, SEL, SEL))objc_msgSend)(proxy, sel_registerName("respondsToSelector:"), kbSel)) {
        int64_t kt = ((int64_t (*)(id, SEL))objc_msgSend)(proxy, kbSel);
        /* URL=3, NumberPad=4, PhonePad=5, NamePhonePad=6, EmailAddress=7, DecimalPad=8, ASCIICapableNumberPad=11 */
        if (kt == 3 || kt == 4 || kt == 5 || kt == 6 || kt == 7 || kt == 8 || kt == 11) {
            return true;
        }
    }
    return false;
}

bool juxu_ios_proxy_is_autospace_allowed(void *proxy_ptr)
{
    id proxy = (id)proxy_ptr;
    if (proxy == nil) return true;
    SEL secSel = sel_registerName("isSecureTextEntry");
    if (((bool (*)(id, SEL, SEL))objc_msgSend)(proxy, sel_registerName("respondsToSelector:"), secSel)) {
        if (((bool (*)(id, SEL))objc_msgSend)(proxy, secSel)) return false;
    }
    if (juxu_ios_proxy_is_one_time_code(proxy_ptr)) return false;
    SEL kbSel = sel_registerName("keyboardType");
    if (((bool (*)(id, SEL, SEL))objc_msgSend)(proxy, sel_registerName("respondsToSelector:"), kbSel)) {
        int64_t kt = ((int64_t (*)(id, SEL))objc_msgSend)(proxy, kbSel);
        /* URL=3, NumberPad=4, PhonePad=5, EmailAddress=7, DecimalPad=8, ASCIICapableNumberPad=11 */
        if (kt == 3 || kt == 4 || kt == 5 || kt == 7 || kt == 8 || kt == 11) {
            return false;
        }
    }
    return true;
}

bool juxu_ios_is_dark_appearance(void *vc_ptr, void *proxy_ptr)
{
    if (proxy_ptr != NULL) {
        SEL kaSel = sel_registerName("keyboardAppearance");
        if (((bool (*)(id, SEL, SEL))objc_msgSend)((id)proxy_ptr, sel_registerName("respondsToSelector:"), kaSel)) {
            /* UIKeyboardAppearanceDark == 1 */
            if (((int64_t (*)(id, SEL))objc_msgSend)((id)proxy_ptr, kaSel) == 1LL) {
                return true;
            }
        }
    }
    if (vc_ptr != NULL) {
        id traits = ((id (*)(id, SEL))objc_msgSend)((id)vc_ptr, sel_registerName("traitCollection"));
        if (traits != nil) {
            /* UIUserInterfaceStyleDark == 2 */
            if (((int64_t (*)(id, SEL))objc_msgSend)(traits, sel_registerName("userInterfaceStyle")) == 2LL) {
                return true;
            }
        }
    }
    return false;
}

double juxu_ios_display_scale(void *view_ptr)
{
    if (view_ptr == NULL) return 2.0;
    id traits = ((id (*)(id, SEL))objc_msgSend)((id)view_ptr, sel_registerName("traitCollection"));
    if (traits == nil) return 2.0;
    double sc = ((double (*)(id, SEL))objc_msgSend)(traits, sel_registerName("displayScale"));
    return sc > 1.0 ? sc : 1.0;
}

double juxu_ios_main_screen_width(void)
{
    Class scrCls = objc_getClass("UIScreen");
    if (scrCls == Nil) return 393.0;
    id mainScr = ((id (*)(id, SEL))objc_msgSend)((id)scrCls, sel_registerName("mainScreen"));
    if (mainScr == nil) return 393.0;
    CGRect b = ((CGRect (*)(id, SEL))objc_msgSend)(mainScr, sel_registerName("bounds"));
    return b.size.width > 0.0 ? b.size.width : 393.0;
}

double juxu_ios_now_seconds_1970(void)
{
    Class dateCls = objc_getClass("NSDate");
    if (dateCls == Nil) return 0.0;
    id now = ((id (*)(id, SEL))objc_msgSend)((id)dateCls, sel_registerName("date"));
    return ((double (*)(id, SEL))objc_msgSend)(now, sel_registerName("timeIntervalSince1970"));
}

int64_t juxu_ios_now_ms_1970(void)
{
    return (int64_t)(juxu_ios_now_seconds_1970() * 1000.0);
}

void *juxu_ios_extract_sorted_touches(void *ui_set_ptr)
{
    id setObj = (id)ui_set_ptr;
    if (setObj == nil) return NULL;
    id all = ((id (*)(id, SEL))objc_msgSend)(setObj, sel_registerName("allObjects"));
    if (all == nil) return NULL;
    int64_t cnt = ((int64_t (*)(id, SEL))objc_msgSend)(all, sel_registerName("count"));
    if (cnt <= 1) return (void *)all;

    id buf[32];
    int64_t n = cnt < 32 ? cnt : 32;
    SEL objAtSel = sel_registerName("objectAtIndex:");
    SEL tsSel = sel_registerName("timestamp");
    double ts[32];
    for (int64_t i = 0; i < n; i++) {
        buf[i] = ((id (*)(id, SEL, uint64_t))objc_msgSend)(all, objAtSel, (uint64_t)i);
        ts[i] = ((double (*)(id, SEL))objc_msgSend)(buf[i], tsSel);
    }
    /* Insertion sort ascending by `touch.timestamp` (preserving stability) */
    for (int64_t i = 1; i < n; i++) {
        id keyObj = buf[i];
        double keyTs = ts[i];
        int64_t j = i - 1;
        while (j >= 0 && ts[j] > keyTs) {
            buf[j + 1] = buf[j];
            ts[j + 1] = ts[j];
            j--;
        }
        buf[j + 1] = keyObj;
        ts[j + 1] = keyTs;
    }
    return (void *)make_array_objects(buf, (uint64_t)n);
}

void juxu_ios_disable_container_touch_delays(void *start_view_ptr)
{
    id cur = (id)start_view_ptr;
    SEL grsSel = sel_registerName("gestureRecognizers");
    SEL cntSel = sel_registerName("count");
    SEL objSel = sel_registerName("objectAtIndex:");
    SEL getDelSel = sel_registerName("delaysTouchesBegan");
    SEL setDelSel = sel_registerName("setDelaysTouchesBegan:");
    SEL supSel = sel_registerName("superview");

    while (cur != nil) {
        id grs = ((id (*)(id, SEL))objc_msgSend)(cur, grsSel);
        if (grs != nil) {
            int64_t n = ((int64_t (*)(id, SEL))objc_msgSend)(grs, cntSel);
            for (int64_t i = 0; i < n; i++) {
                id gr = ((id (*)(id, SEL, uint64_t))objc_msgSend)(grs, objSel, (uint64_t)i);
                if (gr != nil && ((bool (*)(id, SEL))objc_msgSend)(gr, getDelSel)) {
                    ((void (*)(id, SEL, bool))objc_msgSend)(gr, setDelSel, false);
                }
            }
        }
        cur = ((id (*)(id, SEL))objc_msgSend)(cur, supSel);
    }
}

void juxu_ios_suppress_ios26_scroll_edge_effects(void *scroll_view_ptr)
{
    id sv = (id)scroll_view_ptr;
    if (sv == nil) return;
    const char *names[4] = { "topEdgeEffect", "bottomEdgeEffect", "leftEdgeEffect", "rightEdgeEffect" };
    SEL respSel = sel_registerName("respondsToSelector:");
    SEL setHidSel = sel_registerName("setHidden:");
    for (int i = 0; i < 4; i++) {
        SEL effSel = sel_registerName(names[i]);
        if (((bool (*)(id, SEL, SEL))objc_msgSend)(sv, respSel, effSel)) {
            id eff = ((id (*)(id, SEL))objc_msgSend)(sv, effSel);
            if (eff != nil && ((bool (*)(id, SEL, SEL))objc_msgSend)(eff, respSel, setHidSel)) {
                ((void (*)(id, SEL, bool))objc_msgSend)(eff, setHidSel, true);
            }
        }
    }
}

void juxu_ios_open_settings_url(void *vc_ptr)
{
    if (vc_ptr == NULL) return;
    Class urlCls = objc_getClass("NSURL");
    if (urlCls == Nil) return;
    id url = ((id (*)(id, SEL, id))objc_msgSend)(
        (id)urlCls, sel_registerName("URLWithString:"), make_nsstring_utf8("juxu://settings")
    );
    if (url == nil) return;

    Class appCls = objc_getClass("UIApplication");
    SEL kindSel = sel_registerName("isKindOfClass:");
    SEL nextSel = sel_registerName("nextResponder");
    SEL openAppSel = sel_registerName("openURL:options:completionHandler:");
    id emptyDict = ((id (*)(id, SEL))objc_msgSend)((id)objc_getClass("NSDictionary"), sel_registerName("dictionary"));

    id cur = (id)vc_ptr;
    while (cur != nil) {
        if (appCls != Nil && ((bool (*)(id, SEL, Class))objc_msgSend)(cur, kindSel, appCls)) {
            ((void (*)(id, SEL, id, id, void *))objc_msgSend)(cur, openAppSel, url, emptyDict, NULL);
            return;
        }
        cur = ((id (*)(id, SEL))objc_msgSend)(cur, nextSel);
    }

    id extCtx = ((id (*)(id, SEL))objc_msgSend)((id)vc_ptr, sel_registerName("extensionContext"));
    if (extCtx != nil) {
        ((void (*)(id, SEL, id, void *))objc_msgSend)(extCtx, sel_registerName("openURL:completionHandler:"), url, NULL);
    }
}

void *juxu_ios_msgSendSuper_hitTest(void *receiver, const char *super_cls_name, void *sel, double px, double py, void *ev)
{
    if (receiver == NULL || super_cls_name == NULL || sel == NULL) return NULL;
    Class supCls = objc_getClass(super_cls_name);
    if (supCls == Nil) return NULL;
    struct objc_super sup = { (id)receiver, supCls };
    typedef id (*fn_t)(struct objc_super *, SEL, CGPoint, id);
    return (void *)((fn_t)objc_msgSendSuper)(&sup, (SEL)sel, CGPointMake(px, py), (id)ev);
}

/* ---- CoreHaptics Shared Driver (`SharedCoreHapticsDriver` 1:1) ---- */

static bool g_haptics_initialized = false;
static id g_haptic_engine = nil;
static id g_key_tap_player = nil;
static id g_soft_tick_player = nil;
static bool g_haptic_running = false;
static int g_haptic_clients = 0;

static void juxu_haptics_rebuild_players_and_start(void);

typedef struct juxu_haptic_stop_blk {
    void *isa;
    int flags;
    int reserved;
    void *invoke;
    void *descriptor;
} juxu_haptic_stop_blk_t;

static juxu_ios_simple_desc_t g_haptic_cb_desc = { 0, sizeof(juxu_haptic_stop_blk_t) };

static void juxu_haptics_on_stopped_main(void *ctx)
{
    int64_t reason = (int64_t)(intptr_t)ctx;
    /* CHHapticEngineStoppedReasonIdleTimeout == 3 */
    if (reason != 3LL) {
        g_haptic_running = false;
    }
}

static void juxu_haptics_stopped_invoke(juxu_haptic_stop_blk_t *b, int64_t reason)
{
    (void)b;
    dispatch_async_f(dispatch_get_main_queue(), (void *)(intptr_t)reason, juxu_haptics_on_stopped_main);
}

static void juxu_haptics_on_reset_main(void *ctx)
{
    (void)ctx;
    juxu_haptics_rebuild_players_and_start();
}

static void juxu_haptics_reset_invoke(juxu_haptic_stop_blk_t *b)
{
    (void)b;
    dispatch_async_f(dispatch_get_main_queue(), NULL, juxu_haptics_on_reset_main);
}

static id make_transient_player(id eng, float intensity, float sharpness)
{
    Class paramCls = objc_getClass("CHHapticEventParameter");
    Class evCls = objc_getClass("CHHapticEvent");
    Class patCls = objc_getClass("CHHapticPattern");
    if (paramCls == Nil || evCls == Nil || patCls == Nil) return nil;

    id pInt = ((id (*)(id, SEL, id, float))objc_msgSend)(
        ((id (*)(id, SEL))objc_msgSend)((id)paramCls, sel_registerName("alloc")),
        sel_registerName("initWithParameterID:value:"),
        CHHapticEventParameterIDHapticIntensity,
        intensity
    );
    id pSharp = ((id (*)(id, SEL, id, float))objc_msgSend)(
        ((id (*)(id, SEL))objc_msgSend)((id)paramCls, sel_registerName("alloc")),
        sel_registerName("initWithParameterID:value:"),
        CHHapticEventParameterIDHapticSharpness,
        sharpness
    );
    id params[2] = { pInt, pSharp };
    id paramArr = make_array_objects(params, 2);

    id ev = ((id (*)(id, SEL, id, id, double))objc_msgSend)(
        ((id (*)(id, SEL))objc_msgSend)((id)evCls, sel_registerName("alloc")),
        sel_registerName("initWithEventType:parameters:relativeTime:"),
        CHHapticEventTypeHapticTransient,
        paramArr,
        0.0
    );
    id events[1] = { ev };
    id evArr = make_array_objects(events, 1);
    id emptyArr = ((id (*)(id, SEL))objc_msgSend)((id)objc_getClass("NSArray"), sel_registerName("array"));

    id err = nil;
    id pat = ((id (*)(id, SEL, id, id, id *))objc_msgSend)(
        ((id (*)(id, SEL))objc_msgSend)((id)patCls, sel_registerName("alloc")),
        sel_registerName("initWithEvents:parameters:error:"),
        evArr,
        emptyArr,
        &err
    );
    ((void (*)(id, SEL))objc_msgSend)(pInt, sel_registerName("release"));
    ((void (*)(id, SEL))objc_msgSend)(pSharp, sel_registerName("release"));
    ((void (*)(id, SEL))objc_msgSend)(ev, sel_registerName("release"));
    if (pat == nil || err != nil) {
        if (pat != nil) ((void (*)(id, SEL))objc_msgSend)(pat, sel_registerName("release"));
        return nil;
    }

    id playerErr = nil;
    id player = ((id (*)(id, SEL, id, id *))objc_msgSend)(
        eng, sel_registerName("createPlayerWithPattern:error:"), pat, &playerErr
    );
    ((void (*)(id, SEL))objc_msgSend)(pat, sel_registerName("release"));
    if (player == nil || playerErr != nil) {
        return nil;
    }
    return ((id (*)(id, SEL))objc_msgSend)(player, sel_registerName("retain"));
}

static void juxu_haptics_rebuild_players_and_start(void)
{
    if (g_haptic_engine == nil) return;
    if (g_key_tap_player != nil) {
        ((void (*)(id, SEL))objc_msgSend)(g_key_tap_player, sel_registerName("release"));
        g_key_tap_player = nil;
    }
    if (g_soft_tick_player != nil) {
        ((void (*)(id, SEL))objc_msgSend)(g_soft_tick_player, sel_registerName("release"));
        g_soft_tick_player = nil;
    }
    g_key_tap_player = make_transient_player(g_haptic_engine, 0.45f, 0.88f);
    g_soft_tick_player = make_transient_player(g_haptic_engine, 0.31f, 0.80f);
    if (g_key_tap_player == nil || g_soft_tick_player == nil) {
        g_haptic_running = false;
        return;
    }
    id startErr = nil;
    bool ok = ((bool (*)(id, SEL, id *))objc_msgSend)(
        g_haptic_engine, sel_registerName("startAndReturnError:"), &startErr
    );
    g_haptic_running = (ok && startErr == nil);
}

static void juxu_haptics_ensure_setup(void)
{
    if (g_haptics_initialized) return;
    g_haptics_initialized = true;

    Class engCls = objc_getClass("CHHapticEngine");
    if (engCls == Nil) return;
    id caps = ((id (*)(id, SEL))objc_msgSend)((id)engCls, sel_registerName("capabilitiesForHardware"));
    if (caps == nil || !((bool (*)(id, SEL))objc_msgSend)(caps, sel_registerName("supportsHaptics"))) {
        return;
    }

    id err = nil;
    id eng = ((id (*)(id, SEL, id *))objc_msgSend)(
        ((id (*)(id, SEL))objc_msgSend)((id)engCls, sel_registerName("alloc")),
        sel_registerName("initAndReturnError:"),
        &err
    );
    if (eng == nil || err != nil) {
        if (eng != nil) ((void (*)(id, SEL))objc_msgSend)(eng, sel_registerName("release"));
        return;
    }

    ((void (*)(id, SEL, bool))objc_msgSend)(eng, sel_registerName("setPlaysHapticsOnly:"), true);
    ((void (*)(id, SEL, bool))objc_msgSend)(eng, sel_registerName("setAutoShutdownEnabled:"), true);

    juxu_haptic_stop_blk_t stopStack = {
        _NSConcreteStackBlock, 0, 0, (void *)juxu_haptics_stopped_invoke, &g_haptic_cb_desc
    };
    void *stopHeap = _Block_copy(&stopStack);
    ((void (*)(id, SEL, void *))objc_msgSend)(eng, sel_registerName("setStoppedHandler:"), stopHeap);
    _Block_release(stopHeap);

    juxu_haptic_stop_blk_t resetStack = {
        _NSConcreteStackBlock, 0, 0, (void *)juxu_haptics_reset_invoke, &g_haptic_cb_desc
    };
    void *resetHeap = _Block_copy(&resetStack);
    ((void (*)(id, SEL, void *))objc_msgSend)(eng, sel_registerName("setResetHandler:"), resetHeap);
    _Block_release(resetHeap);

    g_haptic_engine = eng;
    juxu_haptics_rebuild_players_and_start();
}

static bool juxu_haptics_play_player(id player)
{
    if (player == nil || g_haptic_engine == nil) return false;
    if (!g_haptic_running) {
        id err = nil;
        ((bool (*)(id, SEL, id *))objc_msgSend)(g_haptic_engine, sel_registerName("startAndReturnError:"), &err);
        g_haptic_running = (err == nil);
    }
    id pErr = nil;
    /* CHHapticTimeImmediate == 0.0 */
    if (((bool (*)(id, SEL, double, id *))objc_msgSend)(player, sel_registerName("startAtTime:error:"), 0.0, &pErr)
        && pErr == nil) {
        return true;
    }
    id restartErr = nil;
    if (((bool (*)(id, SEL, id *))objc_msgSend)(g_haptic_engine, sel_registerName("startAndReturnError:"), &restartErr)
        && restartErr == nil) {
        g_haptic_running = true;
        id retryErr = nil;
        if (((bool (*)(id, SEL, double, id *))objc_msgSend)(player, sel_registerName("startAtTime:error:"), 0.0, &retryErr)
            && retryErr == nil) {
            return true;
        }
    }
    g_haptic_running = false;
    return false;
}

void juxu_ios_haptics_acquire(void)
{
    juxu_haptics_ensure_setup();
    g_haptic_clients += 1;
    if (g_haptic_engine != nil && !g_haptic_running) {
        id err = nil;
        bool ok = ((bool (*)(id, SEL, id *))objc_msgSend)(
            g_haptic_engine, sel_registerName("startAndReturnError:"), &err
        );
        g_haptic_running = (ok && err == nil);
    }
}

void juxu_ios_haptics_release(void)
{
    if (g_haptic_clients > 0) g_haptic_clients -= 1;
    if (g_haptic_clients == 0 && g_haptic_engine != nil) {
        ((void (*)(id, SEL, void *))objc_msgSend)(g_haptic_engine, sel_registerName("stopWithCompletionHandler:"), NULL);
        g_haptic_running = false;
    }
}

bool juxu_ios_haptics_play_key_tap(void)
{
    juxu_haptics_ensure_setup();
    return juxu_haptics_play_player(g_key_tap_player);
}

bool juxu_ios_haptics_play_soft_tick(void)
{
    juxu_haptics_ensure_setup();
    return juxu_haptics_play_player(g_soft_tick_player);
}

#else

/* ---- Non-iOS (`macos` unit-test host) Safe No-Op Stubs ---- */

void juxu_ios_set_continuous_corner(void *l, double r) { (void)l; (void)r; }
void juxu_ios_set_top_masked_corners(void *l) { (void)l; }
void juxu_ios_catransaction_begin_no_actions(void) {}
void juxu_ios_catransaction_commit(void) {}
void juxu_ios_init_edge_fade_layer(void *g) { (void)g; }
void juxu_ios_apply_edge_fade_mask(void *g, void *c, bool l, bool r, double w, double h) {
    (void)g; (void)c; (void)l; (void)r; (void)w; (void)h;
}
void *juxu_ios_color_rgba(double r, double g, double b, double a) {
    (void)r; (void)g; (void)b; (void)a; return NULL;
}
void *juxu_ios_color_white(double w, double a) { (void)w; (void)a; return NULL; }
void *juxu_ios_color_system_blue(double a) { (void)a; return NULL; }
void *juxu_ios_color_system_bg_alpha(double a) { (void)a; return NULL; }
void *juxu_ios_color_label_alpha(double a) { (void)a; return NULL; }
void *juxu_ios_cgcolor(void *c) { (void)c; return NULL; }
void *juxu_ios_system_font(double s, int w) { (void)s; (void)w; return NULL; }
void *juxu_ios_monospaced_font(double s, int w) { (void)s; (void)w; return NULL; }
void *juxu_ios_monospaced_digit_font(double s, int w) { (void)s; (void)w; return NULL; }
void *juxu_ios_symbol_image(const char *n, double p, int w, bool t) {
    (void)n; (void)p; (void)w; (void)t; return NULL;
}
double juxu_ios_measure_text_width(const char *t, void *f) { (void)t; (void)f; return 0.0; }
double juxu_ios_set_dual_attributed_label(
    void *l, const char *p, void *pf, void *pc, const char *s, void *sf, void *sc
) {
    (void)l; (void)p; (void)pf; (void)pc; (void)s; (void)sf; (void)sc; return 0.0;
}
void juxu_ios_set_radical_attributed_label(
    void *l, const char *r, double f, void *c, double m, double sp
) {
    (void)l; (void)r; (void)f; (void)c; (void)m; (void)sp;
}
void juxu_ios_perform_without_animation(void *tramp, int64_t h) {
    if (tramp != NULL) ((void (*)(int64_t))tramp)(h);
    if (h != 0) gc_handle_free(h);
}
void juxu_ios_animate(double d, uint64_t o, void *at, int64_t ah, void *ct, int64_t ch) {
    (void)d; (void)o;
    if (at != NULL) ((void (*)(int64_t))at)(ah);
    if (ah != 0) gc_handle_free(ah);
    if (ct != NULL) ((void (*)(int64_t, bool))ct)(ch, true);
    if (ch != 0) gc_handle_free(ch);
}
void juxu_ios_animate_spring(double d, double dp, double v, uint64_t o, void *at, int64_t ah) {
    (void)d; (void)dp; (void)v; (void)o;
    if (at != NULL) ((void (*)(int64_t))at)(ah);
    if (ah != 0) gc_handle_free(ah);
}
uint64_t juxu_ios_accessibility_trait_button(void) { return 1ULL; }
void *juxu_ios_pasteboard_changed_notification_name(void) { return NULL; }
void juxu_ios_register_style_trait_observer(void *v, void *t, int64_t h) {
    (void)v; (void)t; if (h != 0) gc_handle_free(h);
}
bool juxu_ios_proxy_is_one_time_code(void *p) { (void)p; return false; }
bool juxu_ios_proxy_is_sensitive_eval(void *p) { (void)p; return false; }
bool juxu_ios_proxy_is_autospace_allowed(void *p) { (void)p; return true; }
bool juxu_ios_is_dark_appearance(void *v, void *p) { (void)v; (void)p; return false; }
double juxu_ios_display_scale(void *v) { (void)v; return 2.0; }
double juxu_ios_main_screen_width(void) { return 393.0; }
double juxu_ios_now_seconds_1970(void) { return 0.0; }
int64_t juxu_ios_now_ms_1970(void) { return 0LL; }
void *juxu_ios_extract_sorted_touches(void *s) { (void)s; return NULL; }
void juxu_ios_disable_container_touch_delays(void *v) { (void)v; }
void juxu_ios_suppress_ios26_scroll_edge_effects(void *s) { (void)s; }
void juxu_ios_open_settings_url(void *v) { (void)v; }
void *juxu_ios_msgSendSuper_hitTest(void *r, const char *c, void *s, double x, double y, void *e) {
    (void)r; (void)c; (void)s; (void)x; (void)y; (void)e; return NULL;
}
void juxu_ios_haptics_acquire(void) {}
void juxu_ios_haptics_release(void) {}
bool juxu_ios_haptics_play_key_tap(void) { return false; }
bool juxu_ios_haptics_play_soft_tick(void) { return false; }

#endif
