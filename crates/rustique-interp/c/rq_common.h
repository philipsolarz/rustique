/* Rustique interp tier: state shared by the override support code (rq_isinstance.h,
 * rq_attrcache.h, rq_calls.h). Included by rq_handlers.c after CPython's ceval.h and
 * ceval_macros.h of the patch release being compiled.
 *
 * Concurrency model (docs/targets/interp.md, "Free-threading"):
 *  - every fast path first checks tstate->interp == rq_main_interp. The core's dispatch table
 *    is a static of the binary, so swapped handlers also run in subinterpreters; caches keyed
 *    by type version tags are per-interpreter facts, so only the main interpreter uses them.
 *  - GIL builds: everything below is touched only by threads of the main interpreter holding
 *    its GIL.
 *  - free-threaded builds: counters use relaxed atomic loads and stores (lost updates are
 *    acceptable for a heuristic; no data race), cache entries are seqlocked (rq_attrcache.h)
 *    or packed into one 64-bit word (rq_isinstance.h), and references are obtained with the
 *    primitives CPython's own generic paths use (_Py_TryXGetRef, PyDict_GetItemRef,
 *    _PyType_LookupStackRefAndVersion). */
#ifndef RQ_COMMON_H
#define RQ_COMMON_H

#include <stdint.h>

static PyInterpreterState *rq_main_interp;

/* The core interpreter's own dispatch tables (rq_init): DISPATCH_TABLE and
 * TRACING_DISPATCH_TABLE of ceval_macros.h are redefined to these in rq_handlers.c, so a
 * swapped handler re-dispatches (DISPATCH_SAME_OPARG, DISPATCH_GOTO_NON_TRACING,
 * LEAVE_TRACING) and tests IS_JIT_TRACING() exactly as the core's own handler does. */
static py_tail_call_funcptr *rq_core_handler_table;
static py_tail_call_funcptr *rq_core_tracing_table;

#ifdef Py_GIL_DISABLED
#  define RQ_LD(x) __atomic_load_n(&(x), __ATOMIC_RELAXED)
#  define RQ_ST(x, v) __atomic_store_n(&(x), (v), __ATOMIC_RELAXED)
#else
#  define RQ_LD(x) (x)
#  define RQ_ST(x, v) ((x) = (v))
#endif

#ifdef Py_GIL_DISABLED
/* A strong reference to the MRO lookup of `name` on tp and the version tag it belongs to, or
 * 0. Free-threaded builds only (a borrowed type-dict entry may be freed by a concurrent class
 * mutation). Uses the exported _PyType_LookupRef bracketed by two acquire loads of the tag: a
 * class mutation resets the tag (type_modified_unlocked) before it changes the type's dict
 * (type_update_dict), so a lookup that saw a changed dict sees a different tag afterwards.
 * (Not _PyType_LookupStackRefAndVersion: it is not exported, and calling an LTO-internalized
 * function by address is not safe; its calling convention may have been changed.) */
static int
rq_lookup_ref(PyTypeObject *tp, PyObject *name, PyObject **descr, unsigned int *version)
{
    unsigned int v1 = _Py_atomic_load_uint32_acquire(&tp->tp_version_tag);
    if (v1 == 0) {
        return 0;
    }
    PyObject *d = _PyType_LookupRef(tp, name);
    if (_Py_atomic_load_uint32_acquire(&tp->tp_version_tag) != v1) {
        Py_XDECREF(d);
        return 0;
    }
    *descr = d;
    *version = v1;
    return 1;
}
#endif

/* ---- Override groups and net-benefit suspension (ACCELERATION 5.6 for handlers) ------------
 *
 * Each group counts executions of its overridden path and fast-path hits; attribute-cache
 * fills are excluded from the steady executions (they are the warm-up). Two decisions:
 *  - early, after the first window of RQ_WINDOW executions: a group whose hits are below
 *    1/(RQ_EARLY_FACTOR x ratio) of its steady executions -- far below break-even, its cost is
 *    already visible -- gives the core's handlers back;
 *  - final, after RQ_DECIDE windows (about 1M executions), the same test at 1/ratio: a horizon
 *    long enough to average out start-up phases for borderline groups.
 * Suspending asks the registry (Counters::request_suspend through rq_on_suspend), which
 * restores the core's handlers and reports the target `suspended`; only which, equivalent,
 * handler runs changes. A window that ends while the group is suspended means the registry
 * reinstalled it (enable() after a suspension): the group starts counting afresh.
 *   attr      ratio 5   our copies of the LOAD_ATTR family cost ~0.8% when nothing hits
 *   setattr   ratio 64  a hit saves ~570 Ir, a non-hit costs ~10
 *   kwinit    ratio 64  a hit saves ~1,100 Ir
 *   isinstance ratio 4  a hit saves 27-490 Ir, a fallback (ABCs, hooks, unions) costs +12-45 Ir:
 *                       ABC-heavy code gives the core's handler back (task_planner) */
enum { RQ_G_ATTR = 0, RQ_G_SETATTR = 1, RQ_G_KWINIT = 2, RQ_G_ISINSTANCE = 3, RQ_G_GETATTR = 4,
       RQ_NGROUPS = 5 };
#define RQ_WINDOW 65536u
#define RQ_DECIDE 16
#define RQ_EARLY_FACTOR 4
#define RQ_MAXOPS 64

typedef struct {
    unsigned long long total, hits, fills;   /* cumulative, steady state after decision */
    unsigned int window;                     /* executions in the current window */
    int windows;                             /* windows completed since arming */
    int armed, suspended;
    unsigned int ratio;                      /* 0: never suspended */
    int n;
    void **cell[RQ_MAXOPS];                  /* &core_table[op] */
    void *orig[RQ_MAXOPS];                   /* the core's handler */
    void *mine[RQ_MAXOPS];                   /* ours */
} rq_group_t;

static rq_group_t rq_groups[RQ_NGROUPS] = {
    [RQ_G_ATTR] = {.ratio = 5},
    [RQ_G_SETATTR] = {.ratio = 64},
    [RQ_G_KWINIT] = {.ratio = 64},
    [RQ_G_ISINSTANCE] = {.ratio = 4},
    [RQ_G_GETATTR] = {.ratio = 0},
};

/* Called (any thread, attached) after a group restored the core's handlers. */
static void (*rq_on_suspend)(int group);

static void
rq_group_reset(rq_group_t *g)
{
    RQ_ST(g->total, 0ull);
    RQ_ST(g->hits, 0ull);
    RQ_ST(g->fills, 0ull);
    RQ_ST(g->window, 0u);
    RQ_ST(g->windows, 0);
}

static Py_NO_INLINE void
rq_window_end(rq_group_t *g)
{
    RQ_ST(g->total, RQ_LD(g->total) + RQ_LD(g->window));
    RQ_ST(g->window, 0u);
    if (!RQ_LD(g->armed) || g->ratio == 0) {
        return;
    }
    if (RQ_LD(g->suspended)) {
        /* A full window after the suspension: the registry reinstalled the handlers. */
        rq_group_reset(g);
        RQ_ST(g->suspended, 0);
        return;
    }
    int w = RQ_LD(g->windows) + 1;
    RQ_ST(g->windows, w);
    if (w != 1 && w != RQ_DECIDE) {
        return;
    }
    unsigned long long total = RQ_LD(g->total), fills = RQ_LD(g->fills);
    unsigned long long steady = total > fills ? total - fills : 0;
    unsigned long long need = (unsigned long long)g->ratio * (w == 1 ? RQ_EARLY_FACTOR : 1);
    if (RQ_LD(g->hits) * need >= steady) {
        return;
    }
    RQ_ST(g->suspended, 1);
    if (rq_on_suspend != NULL) {
        rq_on_suspend((int)(g - rq_groups));   /* the registry restores the core's handlers */
        return;
    }
    /* No registry hook: restore the core's handlers here. A thread still running our handler
     * finishes it and dispatches onward through the table: the code stays mapped. */
    for (int i = 0; i < g->n; i++) {
        void *expect = g->mine[i];
        __atomic_compare_exchange_n(g->cell[i], &expect, g->orig[i], 0, __ATOMIC_ACQ_REL,
                                    __ATOMIC_RELAXED);
    }
}

#define RQ_TICK(gi) do { \
        rq_group_t *g_ = &rq_groups[gi]; \
        unsigned int w_ = RQ_LD(g_->window) + 1; \
        RQ_ST(g_->window, w_); \
        if (w_ >= RQ_WINDOW) { rq_window_end(g_); } \
    } while (0)
#define RQ_HIT(gi) RQ_ST(rq_groups[gi].hits, RQ_LD(rq_groups[gi].hits) + 1)

#endif
