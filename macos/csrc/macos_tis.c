/*
 * macOS Carbon TIS / CGEvent / SecureEventInput / QuartzCore C11 Bridge
 * for `juxu_macos` (`Juxu.app`).
 */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/time.h>
#include <unistd.h>

#if defined(__APPLE__)

#include <ApplicationServices/ApplicationServices.h>
#include <Carbon/Carbon.h>
#include <CoreFoundation/CoreFoundation.h>
#include <CoreGraphics/CoreGraphics.h>
#include <objc/message.h>
#include <objc/objc.h>
#include <objc/runtime.h>

int64_t juxu_wall_now_ms(void)
{
    struct timeval tv;
    if (gettimeofday(&tv, NULL) != 0) {
        return 0;
    }
    return (int64_t)tv.tv_sec * 1000LL + (int64_t)(tv.tv_usec / 1000);
}

int64_t juxu_floating_window_base_level(void)
{
    return (int64_t)CGWindowLevelForKey(kCGFloatingWindowLevelKey) + 2LL;
}

bool juxu_is_secure_event_input_enabled(void)
{
    return IsSecureEventInputEnabled() ? true : false;
}

bool juxu_cg_preflight_post_event_access(void)
{
    return CGPreflightPostEventAccess();
}

void juxu_post_virtual_key(int32_t key_code)
{
    CGEventRef down = CGEventCreateKeyboardEvent(NULL, (CGKeyCode)key_code, true);
    CGEventRef up = CGEventCreateKeyboardEvent(NULL, (CGKeyCode)key_code, false);
    if (down != NULL && up != NULL) {
        CGEventPost(kCGHIDEventTap, down);
        CGEventPost(kCGHIDEventTap, up);
    }
    if (down != NULL) CFRelease(down);
    if (up != NULL) CFRelease(up);
}

bool juxu_has_intervening_hid_input(double down_timestamp, double now_uptime)
{
    double elapsed = now_uptime - down_timestamp;
    if (elapsed <= 0.0) {
        return false;
    }
    const CGEventType monitored[5] = {
        kCGEventKeyDown,
        kCGEventLeftMouseDown,
        kCGEventRightMouseDown,
        kCGEventOtherMouseDown,
        kCGEventScrollWheel,
    };
    for (int i = 0; i < 5; i++) {
        CFTimeInterval since_last = CGEventSourceSecondsSinceLastEventType(
            kCGEventSourceStateCombinedSessionState,
            monitored[i]
        );
        if ((double)since_last <= elapsed) {
            return true;
        }
    }
    return false;
}

void juxu_ns_beep(void)
{
    Class soundCls = objc_getClass("NSSound");
    if (soundCls != Nil) {
        typedef void (*beep_fn)(id, SEL);
        ((beep_fn)objc_msgSend)((id)soundCls, sel_registerName("beep"));
    }
}

void *juxu_msgSendSuper_init3ptr(
    void *receiver,
    const char *super_cls_name,
    void *sel,
    void *a0,
    void *a1,
    void *a2
)
{
    if (receiver == NULL || super_cls_name == NULL || sel == NULL) return NULL;
    Class supCls = objc_getClass(super_cls_name);
    if (supCls == Nil) return receiver;
    struct objc_super sup = { (id)receiver, supCls };
    typedef id (*fn_t)(struct objc_super *, SEL, id, id, id);
    return (void *)((fn_t)objc_msgSendSuper)(&sup, (SEL)sel, (id)a0, (id)a1, (id)a2);
}

void juxu_msgSendSuper_void1size(
    void *receiver,
    const char *super_cls_name,
    void *sel,
    double w,
    double h
)
{
    if (receiver == NULL || super_cls_name == NULL || sel == NULL) return;
    Class supCls = objc_getClass(super_cls_name);
    if (supCls == Nil) return;
    struct objc_super sup = { (id)receiver, supCls };
    CGSize sz = { w, h };
    typedef void (*fn_t)(struct objc_super *, SEL, CGSize);
    ((fn_t)objc_msgSendSuper)(&sup, (SEL)sel, sz);
}

void juxu_layer_set_continuous_corner(void *layer_ptr, double radius, bool masks_to_bounds)
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

void juxu_layer_set_border_rgba(void *layer_ptr, double width, double r, double g, double b, double a)
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

/* ---- Carbon TIS Registration & Enablement (`main.swift` 1:1) ---- */

static CFDictionaryRef juxu_make_single_kv_dict(CFStringRef key, CFStringRef val)
{
    const void *keys[1] = { key };
    const void *vals[1] = { val };
    return CFDictionaryCreate(
        kCFAllocatorDefault,
        keys,
        vals,
        1,
        &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks
    );
}

static bool juxu_is_input_mode_enabled_active(CFStringRef mode_id)
{
    if (mode_id == NULL) return false;
    CFDictionaryRef filter = juxu_make_single_kv_dict(kTISPropertyInputSourceID, mode_id);
    if (filter == NULL) return false;
    CFArrayRef list = TISCreateInputSourceList(filter, false);
    CFRelease(filter);
    if (list == NULL) return false;

    bool enabled = false;
    CFIndex n = CFArrayGetCount(list);
    for (CFIndex i = 0; i < n; i++) {
        TISInputSourceRef src = (TISInputSourceRef)CFArrayGetValueAtIndex(list, i);
        if (src != NULL) {
            CFBooleanRef prop = (CFBooleanRef)TISGetInputSourceProperty(src, kTISPropertyInputSourceIsEnabled);
            if (prop != NULL && CFBooleanGetValue(prop)) {
                enabled = true;
                break;
            }
        }
    }
    CFRelease(list);
    return enabled;
}

static void juxu_select_active_mode(CFStringRef mode_id)
{
    if (mode_id == NULL) return;
    CFDictionaryRef filter = juxu_make_single_kv_dict(kTISPropertyInputSourceID, mode_id);
    if (filter == NULL) return;
    CFArrayRef list = TISCreateInputSourceList(filter, false);
    CFRelease(filter);
    if (list == NULL) return;
    if (CFArrayGetCount(list) > 0) {
        TISInputSourceRef src = (TISInputSourceRef)CFArrayGetValueAtIndex(list, 0);
        if (src != NULL) {
            (void)TISSelectInputSource(src);
        }
    }
    CFRelease(list);
}

static void juxu_open_keyboard_input_sources_settings(void)
{
    Class urlCls = objc_getClass("NSURL");
    Class strCls = objc_getClass("NSString");
    Class wsCls = objc_getClass("NSWorkspace");
    if (urlCls == Nil || strCls == Nil || wsCls == Nil) return;

    typedef id (*str_fn)(id, SEL, const char *);
    typedef id (*url_fn)(id, SEL, id);
    typedef id (*ws_fn)(id, SEL);
    typedef bool (*open_fn)(id, SEL, id);

    id s = ((str_fn)objc_msgSend)(
        (id)strCls,
        sel_registerName("stringWithUTF8String:"),
        "x-apple.systempreferences:com.apple.Keyboard-Settings.extension?InputSources"
    );
    if (s == nil) return;
    id url = ((url_fn)objc_msgSend)((id)urlCls, sel_registerName("URLWithString:"), s);
    if (url == nil) return;
    id ws = ((ws_fn)objc_msgSend)((id)wsCls, sel_registerName("sharedWorkspace"));
    if (ws != nil) {
        (void)((open_fn)objc_msgSend)(ws, sel_registerName("openURL:"), url);
    }
}

void juxu_tis_register_system_cache(void *bundle_url)
{
    if (bundle_url != NULL) {
        OSStatus st = TISRegisterInputSource((CFURLRef)bundle_url);
        if (st != noErr) {
            fprintf(stderr, "TISRegisterInputSource (system) returned %d\n", (int)st);
        }
    }
    CFArrayRef all = TISCreateInputSourceList(NULL, true);
    if (all != NULL) {
        CFRelease(all);
    }
}

void juxu_tis_register_and_enable(
    void *bundle_url,
    const char *bundle_id_cstr,
    const char *mode_id_cstr,
    bool should_select
)
{
    const char *bid_raw = (bundle_id_cstr != NULL) ? bundle_id_cstr : "im.juxu.inputmethod.Juxu";
    const char *mid_raw = (mode_id_cstr != NULL) ? mode_id_cstr : "im.juxu.inputmethod.Juxu.Hans";
    CFStringRef bundle_id = CFStringCreateWithCString(kCFAllocatorDefault, bid_raw, kCFStringEncodingUTF8);
    CFStringRef mode_id = CFStringCreateWithCString(kCFAllocatorDefault, mid_raw, kCFStringEncodingUTF8);
    if (bundle_id == NULL || mode_id == NULL) {
        if (bundle_id != NULL) CFRelease(bundle_id);
        if (mode_id != NULL) CFRelease(mode_id);
        return;
    }

    if (bundle_url != NULL) {
        OSStatus regStatus = TISRegisterInputSource((CFURLRef)bundle_url);
        if (regStatus != noErr) {
            fprintf(stderr, "TISRegisterInputSource returned %d\n", (int)regStatus);
        }
    }

    /* Fast path for upgrades: already enabled in active input source list */
    if (juxu_is_input_mode_enabled_active(mode_id)) {
        if (should_select) {
            juxu_select_active_mode(mode_id);
        }
        CFRelease(bundle_id);
        CFRelease(mode_id);
        return;
    }

    TISInputSourceRef found_source = NULL;
    for (int attempt = 0; attempt < 20; attempt++) {
        if ((attempt == 5 || attempt == 12) && bundle_url != NULL) {
            (void)TISRegisterInputSource((CFURLRef)bundle_url);
        }

        CFStringRef filter_keys[3] = {
            kTISPropertyBundleID,
            kTISPropertyInputSourceID,
            kTISPropertyInputSourceID,
        };
        CFStringRef filter_vals[3] = {
            bundle_id,
            bundle_id,
            mode_id,
        };

        for (int f = 0; f < 3; f++) {
            CFDictionaryRef dict = juxu_make_single_kv_dict(filter_keys[f], filter_vals[f]);
            if (dict == NULL) continue;
            CFArrayRef list = TISCreateInputSourceList(dict, true);
            CFRelease(dict);
            if (list == NULL) continue;

            CFIndex n = CFArrayGetCount(list);
            for (CFIndex i = 0; i < n; i++) {
                TISInputSourceRef src = (TISInputSourceRef)CFArrayGetValueAtIndex(list, i);
                if (src == NULL) continue;
                (void)TISEnableInputSource(src);
                CFStringRef src_id = (CFStringRef)TISGetInputSourceProperty(src, kTISPropertyInputSourceID);
                if (src_id != NULL && CFEqual(src_id, mode_id)) {
                    if (found_source != NULL) CFRelease(found_source);
                    found_source = (TISInputSourceRef)CFRetain(src);
                }
            }
            CFRelease(list);
        }

        if (found_source != NULL) {
            break;
        }
        usleep(150000);
    }

    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.4, false);

    /* macOS 26/27+ fallback: open Keyboard Settings InputSources sheet and re-send TISEnableInputSource */
    if (!juxu_is_input_mode_enabled_active(mode_id) && found_source != NULL) {
        juxu_open_keyboard_input_sources_settings();
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0, false);
        (void)TISEnableInputSource(found_source);
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.4, false);
    }

    if (should_select && juxu_is_input_mode_enabled_active(mode_id) && found_source != NULL) {
        (void)TISSelectInputSource(found_source);
    }

    if (found_source != NULL) CFRelease(found_source);
    CFRelease(bundle_id);
    CFRelease(mode_id);
}

#endif
