/*
 * Apple Objective-C Super / Grand Central Dispatch / Dynamic Color C11 Bridge
 * for `juxu_shared` (compiles cleanly on macOS, iOS, and iOS Simulator).
 */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

extern void gc_handle_free(int64_t handle);

int64_t juxu_ptr_to_i64(const void *p)
{
    return (int64_t)(uintptr_t)p;
}

void *juxu_i64_to_ptr(int64_t v)
{
    return (void *)(uintptr_t)v;
}

#if defined(__APPLE__)

#include <dispatch/dispatch.h>
#include <objc/message.h>
#include <objc/objc.h>
#include <objc/runtime.h>
#include <TargetConditionals.h>

/* ---- Objective-C `super` message dispatch ---- */

void juxu_msgSendSuper_void0(void *receiver, const char *super_cls_name, void *sel)
{
    if (receiver == NULL || super_cls_name == NULL || sel == NULL) return;
    Class supCls = objc_getClass(super_cls_name);
    if (supCls == Nil) return;
    struct objc_super sup = { (id)receiver, supCls };
    typedef void (*fn_t)(struct objc_super *, SEL);
    ((fn_t)objc_msgSendSuper)(&sup, (SEL)sel);
}

void juxu_msgSendSuper_void1ptr(void *receiver, const char *super_cls_name, void *sel, void *a0)
{
    if (receiver == NULL || super_cls_name == NULL || sel == NULL) return;
    Class supCls = objc_getClass(super_cls_name);
    if (supCls == Nil) return;
    struct objc_super sup = { (id)receiver, supCls };
    typedef void (*fn_t)(struct objc_super *, SEL, id);
    ((fn_t)objc_msgSendSuper)(&sup, (SEL)sel, (id)a0);
}

void juxu_msgSendSuper_void1bool(void *receiver, const char *super_cls_name, void *sel, bool a0)
{
    if (receiver == NULL || super_cls_name == NULL || sel == NULL) return;
    Class supCls = objc_getClass(super_cls_name);
    if (supCls == Nil) return;
    struct objc_super sup = { (id)receiver, supCls };
    typedef void (*fn_t)(struct objc_super *, SEL, bool);
    ((fn_t)objc_msgSendSuper)(&sup, (SEL)sel, a0);
}

bool juxu_msgSendSuper_bool1ptr(void *receiver, const char *super_cls_name, void *sel, void *a0)
{
    if (receiver == NULL || super_cls_name == NULL || sel == NULL) return false;
    Class supCls = objc_getClass(super_cls_name);
    if (supCls == Nil) return false;
    struct objc_super sup = { (id)receiver, supCls };
    typedef bool (*fn_t)(struct objc_super *, SEL, id);
    return ((fn_t)objc_msgSendSuper)(&sup, (SEL)sel, (id)a0);
}

void *juxu_msgSendSuper_id0(void *receiver, const char *super_cls_name, void *sel)
{
    if (receiver == NULL || super_cls_name == NULL || sel == NULL) return NULL;
    Class supCls = objc_getClass(super_cls_name);
    if (supCls == Nil) return NULL;
    struct objc_super sup = { (id)receiver, supCls };
    typedef id (*fn_t)(struct objc_super *, SEL);
    return (void *)((fn_t)objc_msgSendSuper)(&sup, (SEL)sel);
}

/* ---- Grand Central Dispatch (one-shot GC-handle rooted closures) ---- */

typedef struct juxu_gcd_task {
    void (*trampoline)(int64_t);
    int64_t gc_handle;
} juxu_gcd_task_t;

static void juxu_gcd_invoke_and_free(void *ctx)
{
    juxu_gcd_task_t *t = (juxu_gcd_task_t *)ctx;
    if (t == NULL) return;
    void (*tramp)(int64_t) = t->trampoline;
    int64_t h = t->gc_handle;
    free(t);
    if (tramp != NULL) {
        tramp(h);
    }
    if (h != 0) {
        gc_handle_free(h);
    }
}

void juxu_dispatch_main_async(void *trampoline, int64_t gc_handle)
{
    juxu_gcd_task_t *t = (juxu_gcd_task_t *)malloc(sizeof(juxu_gcd_task_t));
    if (t == NULL) {
        if (gc_handle != 0) gc_handle_free(gc_handle);
        return;
    }
    t->trampoline = (void (*)(int64_t))trampoline;
    t->gc_handle = gc_handle;
    dispatch_async_f(dispatch_get_main_queue(), t, juxu_gcd_invoke_and_free);
}

void juxu_dispatch_main_after_ms(int64_t delay_ms, void *trampoline, int64_t gc_handle)
{
    juxu_gcd_task_t *t = (juxu_gcd_task_t *)malloc(sizeof(juxu_gcd_task_t));
    if (t == NULL) {
        if (gc_handle != 0) gc_handle_free(gc_handle);
        return;
    }
    t->trampoline = (void (*)(int64_t))trampoline;
    t->gc_handle = gc_handle;
    int64_t ns = (delay_ms > 0 ? delay_ms : 0) * 1000000LL;
    dispatch_after_f(dispatch_time(DISPATCH_TIME_NOW, ns), dispatch_get_main_queue(), t, juxu_gcd_invoke_and_free);
}

void *juxu_dispatch_queue_serial_new(const char *label)
{
    dispatch_queue_attr_t attr = dispatch_queue_attr_make_with_qos_class(
        DISPATCH_QUEUE_SERIAL, QOS_CLASS_UTILITY, 0
    );
    return (void *)dispatch_queue_create(label ? label : "im.juxu.serial", attr);
}

void juxu_dispatch_async_on(void *queue, void *trampoline, int64_t gc_handle)
{
    if (queue == NULL) {
        juxu_dispatch_main_async(trampoline, gc_handle);
        return;
    }
    juxu_gcd_task_t *t = (juxu_gcd_task_t *)malloc(sizeof(juxu_gcd_task_t));
    if (t == NULL) {
        if (gc_handle != 0) gc_handle_free(gc_handle);
        return;
    }
    t->trampoline = (void (*)(int64_t))trampoline;
    t->gc_handle = gc_handle;
    dispatch_async_f((dispatch_queue_t)queue, t, juxu_gcd_invoke_and_free);
}

void juxu_dispatch_sync_on(void *queue, void *trampoline, int64_t gc_handle)
{
    if (queue == NULL) {
        if (trampoline != NULL) ((void (*)(int64_t))trampoline)(gc_handle);
        if (gc_handle != 0) gc_handle_free(gc_handle);
        return;
    }
    juxu_gcd_task_t *t = (juxu_gcd_task_t *)malloc(sizeof(juxu_gcd_task_t));
    if (t == NULL) {
        if (gc_handle != 0) gc_handle_free(gc_handle);
        return;
    }
    t->trampoline = (void (*)(int64_t))trampoline;
    t->gc_handle = gc_handle;
    dispatch_sync_f((dispatch_queue_t)queue, t, juxu_gcd_invoke_and_free);
}

/* ---- Dynamic Light/Dark Color Providers (UIKit & AppKit) ---- */

extern void *_NSConcreteStackBlock[32];
extern void *_Block_copy(const void *aBlock);
extern void _Block_release(const void *aBlock);

typedef struct juxu_dyn_color_block {
    void *isa;
    int flags;
    int reserved;
    void *invoke;
    void *descriptor;
    double lr, lg, lb, la;
    double dr, dg, db, da;
} juxu_dyn_color_block_t;

typedef struct juxu_simple_block_desc {
    unsigned long reserved;
    unsigned long size;
} juxu_simple_block_desc_t;

static juxu_simple_block_desc_t g_dyn_color_desc = {
    0, sizeof(juxu_dyn_color_block_t)
};

static id juxu_uicolor_dyn_invoke(juxu_dyn_color_block_t *blk, id traits)
{
    Class uiColorCls = objc_getClass("UIColor");
    if (uiColorCls == Nil) return nil;
    int64_t style = 1;
    if (traits != nil) {
        typedef int64_t (*style_fn)(id, SEL);
        style = ((style_fn)objc_msgSend)(traits, sel_registerName("userInterfaceStyle"));
    }
    bool isDark = (style == 2); /* UIUserInterfaceStyleDark == 2 */
    double r = isDark ? blk->dr : blk->lr;
    double g = isDark ? blk->dg : blk->lg;
    double b = isDark ? blk->db : blk->lb;
    double a = isDark ? blk->da : blk->la;
    typedef id (*col_fn)(id, SEL, double, double, double, double);
    return ((col_fn)objc_msgSend)((id)uiColorCls, sel_registerName("colorWithRed:green:blue:alpha:"), r, g, b, a);
}

void *juxu_uicolor_dynamic(
    double lr, double lg, double lb, double la,
    double dr, double dg, double db, double da
)
{
    Class uiColorCls = objc_getClass("UIColor");
    if (uiColorCls == Nil) return NULL;
    juxu_dyn_color_block_t stack_blk = {
        _NSConcreteStackBlock,
        0,
        0,
        (void *)juxu_uicolor_dyn_invoke,
        &g_dyn_color_desc,
        lr, lg, lb, la,
        dr, dg, db, da
    };
    void *heap_blk = _Block_copy(&stack_blk);
    typedef id (*dyn_fn)(id, SEL, void *);
    id color = ((dyn_fn)objc_msgSend)((id)uiColorCls, sel_registerName("colorWithDynamicProvider:"), heap_blk);
    _Block_release(heap_blk);
    return (void *)color;
}

static id juxu_nscolor_dyn_invoke(juxu_dyn_color_block_t *blk, id appearance)
{
    Class nsColorCls = objc_getClass("NSColor");
    if (nsColorCls == Nil) return nil;
    bool isDark = false;
    if (appearance != nil) {
        typedef id (*str_fn)(id, SEL, const char *);
        Class nsStrCls = objc_getClass("NSString");
        id aqua = ((str_fn)objc_msgSend)((id)nsStrCls, sel_registerName("stringWithUTF8String:"), "NSAppearanceNameAqua");
        id darkAqua = ((str_fn)objc_msgSend)((id)nsStrCls, sel_registerName("stringWithUTF8String:"), "NSAppearanceNameDarkAqua");
        Class nsArrCls = objc_getClass("NSArray");
        typedef id (*arr_fn)(id, SEL, id, id, void *);
        id names = ((arr_fn)objc_msgSend)((id)nsArrCls, sel_registerName("arrayWithObjects:"), aqua, darkAqua, NULL);
        typedef id (*match_fn)(id, SEL, id);
        id best = ((match_fn)objc_msgSend)(appearance, sel_registerName("bestMatchFromAppearancesWithNames:"), names);
        if (best != nil) {
            typedef const char *(*utf8_fn)(id, SEL);
            const char *s = ((utf8_fn)objc_msgSend)(best, sel_registerName("UTF8String"));
            if (s != NULL && strstr(s, "Dark") != NULL) {
                isDark = true;
            }
        }
    }
    double r = isDark ? blk->dr : blk->lr;
    double g = isDark ? blk->dg : blk->lg;
    double b = isDark ? blk->db : blk->lb;
    double a = isDark ? blk->da : blk->la;
    typedef id (*col_fn)(id, SEL, double, double, double, double);
    return ((col_fn)objc_msgSend)((id)nsColorCls, sel_registerName("colorWithSRGBRed:green:blue:alpha:"), r, g, b, a);
}

void *juxu_nscolor_dynamic(
    double lr, double lg, double lb, double la,
    double dr, double dg, double db, double da
)
{
    Class nsColorCls = objc_getClass("NSColor");
    if (nsColorCls == Nil) return NULL;
    juxu_dyn_color_block_t stack_blk = {
        _NSConcreteStackBlock,
        0,
        0,
        (void *)juxu_nscolor_dyn_invoke,
        &g_dyn_color_desc,
        lr, lg, lb, la,
        dr, dg, db, da
    };
    void *heap_blk = _Block_copy(&stack_blk);
    typedef id (*dyn_fn)(id, SEL, id, void *);
    id color = ((dyn_fn)objc_msgSend)((id)nsColorCls, sel_registerName("colorWithName:dynamicProvider:"), nil, heap_blk);
    _Block_release(heap_blk);
    return (void *)color;
}

#else

void juxu_msgSendSuper_void0(void *r, const char *c, void *s) { (void)r; (void)c; (void)s; }
void juxu_msgSendSuper_void1ptr(void *r, const char *c, void *s, void *a) { (void)r; (void)c; (void)s; (void)a; }
void juxu_msgSendSuper_void1bool(void *r, const char *c, void *s, bool a) { (void)r; (void)c; (void)s; (void)a; }
bool juxu_msgSendSuper_bool1ptr(void *r, const char *c, void *s, void *a) { (void)r; (void)c; (void)s; (void)a; return false; }
void *juxu_msgSendSuper_id0(void *r, const char *c, void *s) { (void)r; (void)c; (void)s; return NULL; }
void juxu_dispatch_main_async(void *t, int64_t h) { (void)t; (void)h; }
void juxu_dispatch_main_after_ms(int64_t d, void *t, int64_t h) { (void)d; (void)t; (void)h; }
void *juxu_dispatch_queue_serial_new(const char *l) { (void)l; return NULL; }
void juxu_dispatch_async_on(void *q, void *t, int64_t h) { (void)q; (void)t; (void)h; }
void juxu_dispatch_sync_on(void *q, void *t, int64_t h) { (void)q; (void)t; (void)h; }
void *juxu_uicolor_dynamic(double a, double b, double c, double d, double e, double f, double g, double h) {
    (void)a; (void)b; (void)c; (void)d; (void)e; (void)f; (void)g; (void)h; return NULL;
}
void *juxu_nscolor_dynamic(double a, double b, double c, double d, double e, double f, double g, double h) {
    (void)a; (void)b; (void)c; (void)d; (void)e; (void)f; (void)g; (void)h; return NULL;
}

#endif
