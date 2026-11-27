/*
 * macOS Voice AXUIElement Context Capture, Global Permissions, Global Hotkey CGEventTap,
 * and VoiceGlobalCommitter (AX Insertion + Clipboard Snapshot + Cmd+V Fallback).
 * 1:1 port of VoiceContextCapture.swift and VoiceGlobalHotkey.swift.
 */

#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <Carbon/Carbon.h>
#import <Foundation/Foundation.h>

extern void gc_handle_free(int64_t handle);
typedef void (*juxu_voice_ax_cb_t)(
    int64_t handle,
    void *nsAppName, void *nsAppId, void *nsAppType,
    void *nsWinTitle, void *nsWebUrl,
    bool enhancedUI, void *nsRole, bool isEditable,
    bool isBlacklisted, void *nsBlacklistReason, bool isSecureInput,
    void *nsSelected, void *nsBeforeCur, void *nsAfterCur,
    void *nsBeforeArea, void *nsAfterArea, void *nsVisible,
    int64_t elapsedMs, void *nsPrettyJson
);
typedef void (*juxu_voice_hotkey_cb_t)(bool isKeyDown, int64_t keyCode, int64_t modFlags, void *nsEvent);
typedef void (*juxu_voice_commit_cb_t)(int64_t handle, void *nsNotice);

static NSString *jv2_ns(void *p) { return p ? (__bridge NSString *)p : @""; }
static void *jv2_ret(NSString *s) {
    if (!s || s.length == 0) return (__bridge void *)@"";
    NSString *cp = [s copy];
    return (void *)CFAutorelease(CFBridgingRetain(cp));
}

int64_t juxu_voice_read_i64(const void *buf, int64_t off) {
    int64_t v = 0;
    memcpy(&v, (const uint8_t *)buf + off, sizeof(int64_t));
    return v;
}

double juxu_voice_read_f64(const void *buf, int64_t off) {
    double v = 0.0;
    memcpy(&v, (const uint8_t *)buf + off, sizeof(double));
    return v;
}

/* ---- 1. Global Permissions (`VoiceGlobalPermissions`) ---- */

bool juxu_voice_perm_input_monitoring(void) {
    return CGPreflightListenEventAccess() ? true : false;
}

void juxu_voice_perm_request_all(void) {
    if (!CGPreflightListenEventAccess()) {
        (void)CGRequestListenEventAccess();
    }
    NSDictionary *opts = @{(__bridge NSString *)kAXTrustedCheckOptionPrompt: @YES};
    (void)AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)opts);
}

bool juxu_voice_is_secure_event_input(void) {
    return IsSecureEventInputEnabled() ? true : false;
}

void *juxu_voice_frontmost_bundle_and_name(void) {
    NSRunningApplication *app = NSWorkspace.sharedWorkspace.frontmostApplication;
    NSString *bid = app.bundleIdentifier ?: @"";
    NSString *nm = app.localizedName ?: @"";
    return jv2_ret([NSString stringWithFormat:@"%@\t%@", bid, nm]);
}

/* ---- 2. AX Helpers & Async Context Capture (`VoiceContextCapture`) ---- */

static AXUIElementRef jv_ax_copy_elem(AXUIElementRef el, CFStringRef attr) {
    if (!el) return NULL;
    CFTypeRef ref = NULL;
    if (AXUIElementCopyAttributeValue(el, attr, &ref) == kAXErrorSuccess && ref) {
        if (CFGetTypeID(ref) == AXUIElementGetTypeID()) {
            AXUIElementSetMessagingTimeout((AXUIElementRef)ref, 0.25f);
            return (AXUIElementRef)ref;
        }
        CFRelease(ref);
    }
    return NULL;
}

static NSString *jv_ax_str(AXUIElementRef el, CFStringRef attr) {
    if (!el) return nil;
    CFTypeRef ref = NULL;
    if (AXUIElementCopyAttributeValue(el, attr, &ref) == kAXErrorSuccess && ref) {
        NSString *out = nil;
        if (CFGetTypeID(ref) == CFStringGetTypeID()) out = [(__bridge NSString *)ref copy];
        else if ([(__bridge id)ref isKindOfClass:NSURL.class]) out = [((__bridge NSURL *)ref).absoluteString copy];
        CFRelease(ref);
        return out;
    }
    return nil;
}

static BOOL jv_ax_is_editable(AXUIElementRef el, NSString *role) {
    if (!el) return NO;
    Boolean settable = false;
    if (AXUIElementIsAttributeSettable(el, kAXSelectedTextAttribute, &settable) == kAXErrorSuccess && settable) return YES;
    if (AXUIElementIsAttributeSettable(el, kAXValueAttribute, &settable) == kAXErrorSuccess && settable) return YES;
    return [role isEqualToString:@"AXTextField"] || [role isEqualToString:@"AXTextArea"]
        || [role isEqualToString:@"AXComboBox"] || [role isEqualToString:@"AXSearchField"];
}

static void jv_collect_visible_ax(AXUIElementRef node, int depth, int *visited, NSMutableArray<NSString *> *lines, NSUInteger *chars, NSUInteger maxChars, uint64_t deadlineNs) {
    if (!node || depth > 18 || *visited >= 450 || *chars >= maxChars) return;
    if (clock_gettime_nsec_np(CLOCK_UPTIME_RAW) >= deadlineNs) return;
    (*visited)++;
    NSString *role = jv_ax_str(node, kAXRoleAttribute) ?: @"";
    if ([role isEqualToString:@"AXMenuBar"] || [role isEqualToString:@"AXMenu"] || [role isEqualToString:@"AXMenuItem"]
        || [role isEqualToString:@"AXScrollBar"] || [role isEqualToString:@"AXImage"]) return;
    if ([jv_ax_str(node, kAXSubroleAttribute) isEqualToString:@"AXSecureTextField"]) return;

    if ([role isEqualToString:@"AXStaticText"] || [role isEqualToString:@"AXTextField"]
        || [role isEqualToString:@"AXTextArea"] || [role isEqualToString:@"AXHeading"] || [role isEqualToString:@"AXLink"]) {
        NSString *v = jv_ax_str(node, kAXValueAttribute);
        if (v.length == 0 && ([role isEqualToString:@"AXHeading"] || [role isEqualToString:@"AXLink"])) {
            v = jv_ax_str(node, kAXTitleAttribute) ?: jv_ax_str(node, kAXDescriptionAttribute);
        }
        NSString *tr = [v stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        if (tr.length > 0 && ![lines.lastObject isEqualToString:tr]) {
            if (tr.length > 300) tr = [tr substringToIndex:300];
            [lines addObject:tr];
            *chars += tr.length + 1;
        }
        if ([role isEqualToString:@"AXStaticText"] || [role isEqualToString:@"AXTextField"]) return;
    }

    CFTypeRef arrRef = NULL;
    if (AXUIElementCopyAttributeValue(node, kAXVisibleChildrenAttribute, &arrRef) != kAXErrorSuccess || !arrRef) {
        AXUIElementCopyAttributeValue(node, kAXChildrenAttribute, &arrRef);
    }
    if (arrRef) {
        if (CFGetTypeID(arrRef) == CFArrayGetTypeID()) {
            CFIndex cnt = MIN(CFArrayGetCount((CFArrayRef)arrRef), 32);
            for (CFIndex i = 0; i < cnt; i++) {
                CFTypeRef item = CFArrayGetValueAtIndex((CFArrayRef)arrRef, i);
                if (item && CFGetTypeID(item) == AXUIElementGetTypeID()) {
                    AXUIElementSetMessagingTimeout((AXUIElementRef)item, 0.25f);
                    jv_collect_visible_ax((AXUIElementRef)item, depth + 1, visited, lines, chars, maxChars, deadlineNs);
                }
            }
        }
        CFRelease(arrRef);
    }
}

void juxu_voice_capture_context_async(bool respectBlacklist, void *fnPtr, int64_t cbHandle) {
    juxu_voice_ax_cb_t cb = (juxu_voice_ax_cb_t)fnPtr;
    NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
    NSString *appName = front.localizedName ?: @"";
    NSString *bundleId = front.bundleIdentifier ?: @"";
    NSString *appPath = front.bundleURL.path ?: @"";
    pid_t pid = front.processIdentifier;
    BOOL ownApp = (pid == NSProcessInfo.processInfo.processIdentifier && pid != 0);
    BOOL secEvent = IsSecureEventInputEnabled() ? YES : NO;

    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        uint64_t t0 = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
        uint64_t deadline = t0 + 500ULL * 1000000ULL;

        NSSet *hardBlocked = [NSSet setWithArray:@[
            @"com.1password.1password", @"com.agilebits.onepassword7", @"com.agilebits.onepassword-osx",
            @"com.bitwarden.desktop", @"com.apple.keychainaccess", @"com.apple.Passwords",
            @"com.lastpass.LastPass", @"com.dashlane.Dashlane", @"org.keepassxc.keepassxc"
        ]];
        NSSet *terminals = [NSSet setWithArray:@[
            @"com.apple.Terminal", @"com.googlecode.iterm2", @"com.mitchellh.ghostty",
            @"dev.warp.Warp-Stable", @"dev.warp.Warp-Preview", @"dev.warp.Warp",
            @"net.kovidgoyal.kitty", @"org.alacritty", @"io.alacritty", @"com.github.wez.wezterm"
        ]];
        NSSet *browsers = [NSSet setWithArray:@[
            @"com.google.Chrome", @"com.apple.Safari", @"company.thebrowser.Browser",
            @"com.microsoft.edgemac", @"com.brave.Browser", @"org.mozilla.firefox", @"app.zen-browser.zen"
        ]];
        NSSet *softBlack = [NSSet setWithArray:@[
            @"com.sublimetext.4", @"com.tencent.xinWeChat", @"com.microsoft.Excel",
            @"com.kingsoft.wpsoffice.mac", @"dev.zed.Zed"
        ]];

        BOOL isBrowser = [browsers containsObject:bundleId];
        NSString *appType = isBrowser ? @"web_browser" : @"native_app";
        NSString *winTitle = @"";
        NSString *webUrl = nil;
        NSString *role = @"";
        BOOL enhanced = NO, editable = NO, blacklisted = NO, isSecure = secEvent;
        NSString *blReason = nil;
        NSString *selText = @"", *beforeCur = @"", *afterCur = @"", *beforeArea = @"", *afterArea = @"", *visText = nil;

        if ([hardBlocked containsObject:bundleId]) {
            blacklisted = YES; blReason = [@"password_manager:" stringByAppendingString:bundleId];
        } else if ([terminals containsObject:bundleId]) {
            blacklisted = YES; blReason = [@"terminal:" stringByAppendingString:bundleId];
        } else if (respectBlacklist && [softBlack containsObject:bundleId]) {
            blacklisted = YES; blReason = [@"app_blacklist:" stringByAppendingString:bundleId];
        } else if (!secEvent && AXIsProcessTrusted() && pid > 0) {
            AXUIElementRef appEl = AXUIElementCreateApplication(pid);
            AXUIElementSetMessagingTimeout(appEl, 0.25f);
            if (isBrowser || [appPath containsString:@"Electron"]) {
                enhanced = (AXUIElementSetAttributeValue(appEl, CFSTR("AXEnhancedUserInterface"), kCFBooleanTrue) == kAXErrorSuccess);
            }
            AXUIElementRef winEl = jv_ax_copy_elem(appEl, kAXFocusedWindowAttribute);
            if (!winEl) winEl = jv_ax_copy_elem(appEl, kAXMainWindowAttribute);
            if (winEl) winTitle = jv_ax_str(winEl, kAXTitleAttribute) ?: @"";

            NSString *lowTitle = [winTitle lowercaseString];
            if (isBrowser && ([lowTitle containsString:@"incognito"] || [lowTitle containsString:@"private"]
                              || [lowTitle containsString:@"inprivate"] || [lowTitle containsString:@"无痕"]
                              || [lowTitle containsString:@"隐私"])) {
                blacklisted = YES; blReason = [@"private_window:" stringByAppendingString:bundleId];
                winTitle = @"";
            } else {
                AXUIElementRef focEl = jv_ax_copy_elem(appEl, kAXFocusedUIElementAttribute);
                if (focEl) {
                    NSString *subrole = jv_ax_str(focEl, kAXSubroleAttribute) ?: @"";
                    if ([subrole isEqualToString:@"AXSecureTextField"]) {
                        isSecure = YES; winTitle = @"";
                    } else {
                        role = jv_ax_str(focEl, kAXRoleAttribute) ?: @"";
                        editable = jv_ax_is_editable(focEl, role);
                        if (isBrowser && winEl) {
                            NSString *doc = jv_ax_str(winEl, kAXDocumentAttribute);
                            if ([doc hasPrefix:@"http://"] || [doc hasPrefix:@"https://"]) webUrl = doc;
                        }
                        if (respectBlacklist && webUrl &&
                            ([webUrl hasPrefix:@"https://docs.google.com/document/d"]
                             || [webUrl hasPrefix:@"https://docs.qq.com/doc/"]
                             || [webUrl hasPrefix:@"https://docs.qq.com/sheet/"])) {
                            blacklisted = YES; blReason = [@"url_blacklist:" stringByAppendingString:webUrl];
                            winTitle = @""; webUrl = nil;
                        } else if (!ownApp) {
                            selText = jv_ax_str(focEl, kAXSelectedTextAttribute) ?: @"";
                            if (selText.length > 4000) selText = [selText substringToIndex:4000];
                            NSString *fullVal = jv_ax_str(focEl, kAXValueAttribute) ?: @"";
                            CFTypeRef rngRef = NULL;
                            CFRange cfr = CFRangeMake(-1, 0);
                            if (AXUIElementCopyAttributeValue(focEl, kAXSelectedTextRangeAttribute, &rngRef) == kAXErrorSuccess && rngRef) {
                                if (CFGetTypeID(rngRef) == AXValueGetTypeID()) {
                                    AXValueGetValue((AXValueRef)rngRef, kAXValueCFRangeType, &cfr);
                                }
                                CFRelease(rngRef);
                            }
                            if (cfr.location >= 0 && fullVal.length > 0) {
                                NSUInteger loc = MIN((NSUInteger)cfr.location, fullVal.length);
                                NSUInteger len = MIN((NSUInteger)MAX(0, cfr.length), fullVal.length - loc);
                                NSUInteger bStart = loc > 2000 ? loc - 2000 : 0;
                                beforeCur = [fullVal substringWithRange:NSMakeRange(bStart, loc - bStart)];
                                NSUInteger aStart = loc + len;
                                NSUInteger aLen = MIN((NSUInteger)2000, fullVal.length - aStart);
                                afterCur = aLen > 0 ? [fullVal substringWithRange:NSMakeRange(aStart, aLen)] : @"";
                                if (selText.length == 0 && len > 0) {
                                    selText = [fullVal substringWithRange:NSMakeRange(loc, MIN((NSUInteger)4000, len))];
                                }
                            } else if (fullVal.length > 0) {
                                beforeCur = fullVal.length > 2000 ? [fullVal substringFromIndex:fullVal.length - 2000] : fullVal;
                            }
                        }
                    }
                    CFRelease(focEl);
                }
                if (!blacklisted && !isSecure && !ownApp && winEl) {
                    NSMutableArray<NSString *> *vLines = [NSMutableArray array];
                    int visVisited = 0; NSUInteger visChars = 0;
                    jv_collect_visible_ax(winEl, 0, &visVisited, vLines, &visChars, 12000, deadline);
                    if (vLines.count > 0) visText = [vLines componentsJoinedByString:@"\n"];
                }
            }
            if (winEl) CFRelease(winEl);
            if (appEl) CFRelease(appEl);
        }

        int64_t elapsedMs = (int64_t)((clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - t0) / 1000000ULL);
        NSDictionary *jsonDict = @{
            @"active_application": @{
                @"app_name": appName, @"app_identifier": bundleId, @"app_type": appType,
                @"window_title": winTitle, @"web_url": webUrl ?: NSNull.null,
                @"visible_screen_content": visText ?: NSNull.null
            },
            @"text_insertion_point": @{
                @"accessibility_role": role,
                @"input_capabilities": @{@"is_editable": @(editable), @"is_secure": @(isSecure)},
                @"cursor_state": @{
                    @"selected_text": selText, @"text_before_cursor": beforeCur, @"text_after_cursor": afterCur
                },
                @"surrounding_context": @{
                    @"text_before_input_area": beforeArea, @"text_after_input_area": afterArea
                }
            },
            @"context_metadata": @{
                @"is_accessibility_trusted": @(AXIsProcessTrusted() ? YES : NO),
                @"enhanced_ui_enabled": @(enhanced),
                @"is_blacklisted": @(blacklisted),
                @"blacklist_reason": blReason ?: NSNull.null,
                @"is_secure_input": @(isSecure),
                @"elapsed_ms": @(elapsedMs)
            }
        };
        NSData *jData = [NSJSONSerialization dataWithJSONObject:jsonDict options:NSJSONWritingPrettyPrinted | NSJSONWritingSortedKeys error:nil];
        NSString *prettyJson = [[NSString alloc] initWithData:jData encoding:NSUTF8StringEncoding] ?: @"{}";

        dispatch_async(dispatch_get_main_queue(), ^{
            if (cb) {
                cb(cbHandle,
                   jv2_ret(appName), jv2_ret(bundleId), jv2_ret(appType),
                   jv2_ret(winTitle), webUrl ? jv2_ret(webUrl) : NULL,
                   enhanced, jv2_ret(role), editable,
                   blacklisted, blReason ? jv2_ret(blReason) : NULL, isSecure,
                   jv2_ret(selText), jv2_ret(beforeCur), jv2_ret(afterCur),
                   jv2_ret(beforeArea), jv2_ret(afterArea), visText ? jv2_ret(visText) : NULL,
                   elapsedMs, jv2_ret(prettyJson));
            }
            gc_handle_free(cbHandle);
        });
    });
}

/* ---- 3. Global Hotkey Tap (`VoiceGlobalHotkeyController`) & Committer (`VoiceGlobalCommitter`) ---- */

static CFMachPortRef g_event_tap = NULL;
static CFRunLoopSourceRef g_event_src = NULL;
static juxu_voice_hotkey_cb_t g_hotkey_cb = NULL;
static pid_t g_target_pid = 0;
static AXUIElementRef g_target_elem = NULL;

static CGEventRef jv_event_tap_cb(CGEventTapProxy proxy, CGEventType type, CGEventRef event, void *refcon) {
    (void)proxy; (void)refcon;
    if (type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput) {
        if (g_event_tap) CGEventTapEnable(g_event_tap, true);
        return event;
    }
    if ((type == kCGEventFlagsChanged || type == kCGEventKeyDown) && g_hotkey_cb) {
        NSEvent *nsEv = [NSEvent eventWithCGEvent:event];
        if (nsEv) {
            bool isKd = (type == kCGEventKeyDown);
            int64_t kc = (int64_t)nsEv.keyCode;
            int64_t fl = (int64_t)(nsEv.modifierFlags & NSEventModifierFlagDeviceIndependentFlagsMask);
            g_hotkey_cb(isKd, kc, fl, (__bridge void *)nsEv);
        }
    }
    return event;
}

bool juxu_voice_global_tap_start(void *cbFnPtr) {
    g_hotkey_cb = (juxu_voice_hotkey_cb_t)cbFnPtr;
    if (g_event_tap) return true;
    if (!CGPreflightListenEventAccess()) return false;
    CGEventMask mask = CGEventMaskBit(kCGEventFlagsChanged) | CGEventMaskBit(kCGEventKeyDown);
    CFMachPortRef port = CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionListenOnly, mask, jv_event_tap_cb, NULL);
    if (!port) return false;
    CFRunLoopSourceRef src = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, port, 0);
    CFRunLoopAddSource(CFRunLoopGetMain(), src, kCFRunLoopCommonModes);
    CGEventTapEnable(port, true);
    g_event_tap = port;
    g_event_src = src;
    return true;
}

void juxu_voice_global_tap_stop(void) {
    if (g_event_tap) CGEventTapEnable(g_event_tap, false);
    if (g_event_src) { CFRunLoopRemoveSource(CFRunLoopGetMain(), g_event_src, kCFRunLoopCommonModes); CFRelease(g_event_src); g_event_src = NULL; }
    if (g_event_tap) { CFMachPortInvalidate(g_event_tap); CFRelease(g_event_tap); g_event_tap = NULL; }
}

bool juxu_voice_global_tap_is_listening(void) { return g_event_tap != NULL; }

void juxu_voice_committer_capture_target(double *outX, double *outY) {
    if (g_target_elem) { CFRelease(g_target_elem); g_target_elem = NULL; }
    g_target_pid = NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier;
    if (AXIsProcessTrusted()) {
        AXUIElementRef sys = AXUIElementCreateSystemWide();
        AXUIElementSetMessagingTimeout(sys, 0.25f);
        g_target_elem = jv_ax_copy_elem(sys, kAXFocusedUIElementAttribute);
        CFRelease(sys);
    }
    NSPoint mouse = [NSEvent mouseLocation];
    if (outX) *outX = (double)mouse.x;
    if (outY) *outY = (double)mouse.y;
}

void *juxu_voice_committer_selected_text(void) {
    if (IsSecureEventInputEnabled() || !g_target_elem) return NULL;
    if ([jv_ax_str(g_target_elem, kAXSubroleAttribute) isEqualToString:@"AXSecureTextField"]) return NULL;
    NSString *sel = jv_ax_str(g_target_elem, kAXSelectedTextAttribute);
    if (!sel || sel.length == 0 || sel.length > 4000) return NULL;
    return jv2_ret(sel);
}

void juxu_voice_committer_commit(void *nsText, void *cbFnPtr, int64_t cbHandle) {
    NSString *text = jv2_ns(nsText);
    juxu_voice_commit_cb_t cb = (juxu_voice_commit_cb_t)cbFnPtr;
    pid_t origPid = g_target_pid;
    AXUIElementRef origEl = g_target_elem;
    g_target_pid = 0; g_target_elem = NULL;

    void (^done)(NSString *) = ^(NSString *notice) {
        if (origEl) CFRelease(origEl);
        if (cb) cb(cbHandle, notice ? jv2_ret(notice) : NULL);
        gc_handle_free(cbHandle);
    };
    void (^copyOnly)(NSString *) = ^(NSString *notice) {
        NSPasteboard *pb = NSPasteboard.generalPasteboard;
        [pb clearContents];
        [pb setString:text forType:NSPasteboardTypeString];
        done(notice);
    };

    if (IsSecureEventInputEnabled()) { copyOnly(@"密码框不会写入，文字已放到剪贴板"); return; }
    NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
    if (origPid == 0 || front.processIdentifier != origPid) { copyOnly(@"焦点已变化，文字已放到剪贴板"); return; }
    if (!AXIsProcessTrusted()) { copyOnly(@"未授权辅助功能，文字已放到剪贴板"); return; }

    AXUIElementRef sys = AXUIElementCreateSystemWide();
    AXUIElementSetMessagingTimeout(sys, 0.25f);
    AXUIElementRef cur = jv_ax_copy_elem(sys, kAXFocusedUIElementAttribute);
    CFRelease(sys);
    if (cur && [jv_ax_str(cur, kAXSubroleAttribute) isEqualToString:@"AXSecureTextField"]) {
        CFRelease(cur);
        copyOnly(@"密码框不会写入，文字已放到剪贴板");
        return;
    }

    NSString *role = cur ? (jv_ax_str(cur, kAXRoleAttribute) ?: @"") : @"";
    BOOL isTf = cur ? jv_ax_is_editable(cur, role) : NO;
    if (cur && isTf) {
        NSString *before = jv_ax_str(cur, kAXValueAttribute);
        if (before && AXUIElementSetAttributeValue(cur, kAXSelectedTextAttribute, (__bridge CFStringRef)text) == kAXErrorSuccess) {
            NSString *after = jv_ax_str(cur, kAXValueAttribute);
            if (after && ![after isEqualToString:before] && [after containsString:text]) {
                CFRelease(cur);
                done(nil);
                return;
            }
        }
    }

    /* Paste fallback with clipboard snapshot & conditional restore */
    NSPasteboard *pb = NSPasteboard.generalPasteboard;
    NSMutableArray<NSDictionary<NSPasteboardType, NSData *> *> *snap = [NSMutableArray array];
    for (NSPasteboardItem *item in (pb.pasteboardItems ?: @[])) {
        NSMutableDictionary *d = [NSMutableDictionary dictionary];
        for (NSPasteboardType t in item.types) {
            NSData *dat = [item dataForType:t];
            if (dat) d[t] = dat;
        }
        if (d.count > 0) [snap addObject:d];
    }
    NSString *beforeVal = cur ? jv_ax_str(cur, kAXValueAttribute) : nil;
    [pb clearContents];
    [pb setString:text forType:NSPasteboardTypeString];
    NSInteger stamp = pb.changeCount;

    CGEventSourceRef src = CGEventSourceCreate(kCGEventSourceStateHIDSystemState);
    CGEventRef kd = CGEventCreateKeyboardEvent(src, (CGKeyCode)kVK_ANSI_V, true);
    if (kd) { CGEventSetFlags(kd, kCGEventFlagMaskCommand); CGEventPost(kCGHIDEventTap, kd); CFRelease(kd); }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(15 * NSEC_PER_MSEC)), dispatch_get_main_queue(), ^{
        CGEventRef ku = CGEventCreateKeyboardEvent(src, (CGKeyCode)kVK_ANSI_V, false);
        if (ku) { CGEventSetFlags(ku, kCGEventFlagMaskCommand); CGEventPost(kCGHIDEventTap, ku); CFRelease(ku); }
        if (src) CFRelease(src);
    });
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(350 * NSEC_PER_MSEC)), dispatch_get_main_queue(), ^{
        NSString *afterVal = cur ? jv_ax_str(cur, kAXValueAttribute) : nil;
        if (cur) CFRelease(cur);
        BOOL confirmed = (afterVal != nil && ![afterVal isEqualToString:beforeVal] && [afterVal containsString:text]);
        if (confirmed && pb.changeCount == stamp) {
            [pb clearContents];
            NSMutableArray<NSPasteboardItem *> *items = [NSMutableArray array];
            for (NSDictionary<NSPasteboardType, NSData *> *d in snap) {
                NSPasteboardItem *it = [[NSPasteboardItem alloc] init];
                [d enumerateKeysAndObjectsUsingBlock:^(NSPasteboardType k, NSData *v, BOOL *s) { (void)s; [it setData:v forType:k]; }];
                [items addObject:it];
            }
            if (items.count > 0) [pb writeObjects:items];
            done(nil);
        } else {
            done(nil);
        }
    });
}
