#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#if defined(__APPLE__)
#include <ApplicationServices/ApplicationServices.h>
#include <CoreFoundation/CoreFoundation.h>
#include <objc/message.h>
#include <objc/runtime.h>
#include <Security/Security.h>

static const char *nsstr_utf8(void *nsstr) {
    if (!nsstr) return NULL;
    typedef const char *(*fn_t)(id, SEL);
    return ((fn_t)objc_msgSend)((id)nsstr, sel_registerName("UTF8String"));
}

static void *utf8_to_nsstr(const char *cstr) {
    if (!cstr) return NULL;
    Class cls = objc_getClass("NSString");
    if (!cls) return NULL;
    typedef id (*fn_t)(id, SEL, const char *);
    return (void *)((fn_t)objc_msgSend)((id)cls, sel_registerName("stringWithUTF8String:"), cstr);
}

static char *juxu_voice_kc_read_one(const char *service, const char *account) {
    if (!service || !account) return NULL;
    CFStringRef cfSvc = CFStringCreateWithCString(kCFAllocatorDefault, service, kCFStringEncodingUTF8);
    CFStringRef cfAcc = CFStringCreateWithCString(kCFAllocatorDefault, account, kCFStringEncodingUTF8);
    if (!cfSvc || !cfAcc) {
        if (cfSvc) CFRelease(cfSvc);
        if (cfAcc) CFRelease(cfAcc);
        return NULL;
    }
    const void *keys[5] = {
        kSecClass, kSecAttrService, kSecAttrAccount, kSecReturnData, kSecMatchLimit
    };
    const void *vals[5] = {
        kSecClassGenericPassword, cfSvc, cfAcc, kCFBooleanTrue, kSecMatchLimitOne
    };
    CFDictionaryRef query = CFDictionaryCreate(
        kCFAllocatorDefault, keys, vals, 5,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks
    );
    CFTypeRef result = NULL;
    OSStatus st = SecItemCopyMatching(query, &result);
    CFRelease(query);
    CFRelease(cfSvc);
    CFRelease(cfAcc);
    if (st != errSecSuccess || !result) return NULL;
    if (CFGetTypeID(result) != CFDataGetTypeID()) {
        CFRelease(result);
        return NULL;
    }
    CFDataRef data = (CFDataRef)result;
    CFIndex len = CFDataGetLength(data);
    if (len <= 0) {
        CFRelease(result);
        return NULL;
    }
    char *out = (char *)malloc((size_t)len + 1);
    if (out) {
        memcpy(out, CFDataGetBytePtr(data), (size_t)len);
        out[len] = '\0';
    }
    CFRelease(result);
    return out;
}

static bool kc_delete_cstr(const char *account) {
    if (!account) return false;
    const char *svcs[2] = { "im.juxu.voice", "com.hcyue.juxu.voice" };
    bool ok = true;
    for (int i = 0; i < 2; i++) {
        CFStringRef cfSvc = CFStringCreateWithCString(kCFAllocatorDefault, svcs[i], kCFStringEncodingUTF8);
        CFStringRef cfAcc = CFStringCreateWithCString(kCFAllocatorDefault, account, kCFStringEncodingUTF8);
        const void *keys[3] = { kSecClass, kSecAttrService, kSecAttrAccount };
        const void *vals[3] = { kSecClassGenericPassword, cfSvc, cfAcc };
        CFDictionaryRef query = CFDictionaryCreate(
            kCFAllocatorDefault, keys, vals, 3,
            &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks
        );
        OSStatus st = SecItemDelete(query);
        CFRelease(query);
        CFRelease(cfSvc);
        CFRelease(cfAcc);
        if (st != errSecSuccess && st != errSecItemNotFound) ok = false;
    }
    return ok;
}

static bool kc_write_cstr(const char *account, const char *value) {
    if (!account) return false;
    if (!value || value[0] == '\0') {
        return kc_delete_cstr(account);
    }
    CFStringRef cfSvc = CFStringCreateWithCString(kCFAllocatorDefault, "im.juxu.voice", kCFStringEncodingUTF8);
    CFStringRef cfAcc = CFStringCreateWithCString(kCFAllocatorDefault, account, kCFStringEncodingUTF8);
    CFDataRef cfData = CFDataCreate(kCFAllocatorDefault, (const UInt8 *)value, (CFIndex)strlen(value));
    const void *qkeys[3] = { kSecClass, kSecAttrService, kSecAttrAccount };
    const void *qvals[3] = { kSecClassGenericPassword, cfSvc, cfAcc };
    CFDictionaryRef query = CFDictionaryCreate(
        kCFAllocatorDefault, qkeys, qvals, 3,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks
    );
    const void *ukeys[1] = { kSecValueData };
    const void *uvals[1] = { cfData };
    CFDictionaryRef update = CFDictionaryCreate(
        kCFAllocatorDefault, ukeys, uvals, 1,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks
    );
    OSStatus st = SecItemUpdate(query, update);
    CFRelease(update);
    CFRelease(query);
    if (st == errSecItemNotFound) {
        const void *akeys[5] = {
            kSecClass, kSecAttrService, kSecAttrAccount, kSecValueData, kSecAttrAccessible
        };
        const void *avals[5] = {
            kSecClassGenericPassword, cfSvc, cfAcc, cfData, kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        };
        CFDictionaryRef addQ = CFDictionaryCreate(
            kCFAllocatorDefault, akeys, avals, 5,
            &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks
        );
        st = SecItemAdd(addQ, NULL);
        CFRelease(addQ);
    }
    CFRelease(cfData);
    CFRelease(cfAcc);
    CFRelease(cfSvc);
    return st == errSecSuccess;
}

bool juxu_voice_kc_delete_ns(void *nsAccount) {
    return kc_delete_cstr(nsstr_utf8(nsAccount));
}

bool juxu_voice_kc_write_ns(void *nsAccount, void *nsValue) {
    return kc_write_cstr(nsstr_utf8(nsAccount), nsstr_utf8(nsValue));
}

void *juxu_voice_kc_read_ns(void *nsAccount) {
    const char *acc = nsstr_utf8(nsAccount);
    if (!acc) return NULL;
    char *val = juxu_voice_kc_read_one("im.juxu.voice", acc);
    if (!val) {
        val = juxu_voice_kc_read_one("com.hcyue.juxu.voice", acc);
        if (val && val[0] != '\0') {
            kc_write_cstr(acc, val);
        }
    }
    if (!val) return NULL;
    void *ns = utf8_to_nsstr(val);
    free(val);
    return ns;
}

double juxu_voice_cf_absolute_time(void) {
    return (double)CFAbsoluteTimeGetCurrent();
}

bool juxu_voice_ax_is_trusted(void) {
    return AXIsProcessTrusted() ? true : false;
}
#else
bool juxu_voice_kc_delete_ns(void *a) { (void)a; return false; }
bool juxu_voice_kc_write_ns(void *a, void *v) { (void)a; (void)v; return false; }
void *juxu_voice_kc_read_ns(void *a) { (void)a; return NULL; }
double juxu_voice_cf_absolute_time(void) { return 0.0; }
bool juxu_voice_ax_is_trusted(void) { return false; }
#endif
